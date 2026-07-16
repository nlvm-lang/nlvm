//! Signatures for the native template classes `system.List<T>` and
//! `system.Map<K,V>` — stdlib.md § system.List/system.Map, vm.md §
//! Templates (monomorphization). Mirrored by `nl_codegen::native_generics`
//! (kept independent, matching this crate's existing pattern for `system.*`
//! natives — see `stdlib.rs`'s doc comment).
//!
//! Unlike `stdlib.rs`'s flat classes, these are *generic*: by the time
//! nl-sema sees a reference to one, `nl_syntax::monomorphize` has already
//! mangled it down to a concrete instantiation FQCN, e.g.
//! `"system.List<int>"` or `"system.Map<string, int>"` (never a bare
//! `"system.List"` — there is no unparameterized use). This module recovers
//! the concrete element type(s) by parsing that mangled name back apart,
//! rather than carrying them around separately, since the mangled string
//! *is* the only thing available at every call site by then.
//!
//! Only a single level of nesting is exercised/tested: a native generic
//! used as another native/user generic's own type argument (e.g.
//! `system.List<system.List<int>>`) parses in principle (the split is
//! bracket-depth-aware) but is out of scope for PLAN.md Phase 6 and
//! untested. `Map.forEach` is implemented (closures-as-native-callbacks,
//! see `nl_vm::native::dispatch_array`'s doc comment); `List` has no
//! `forEach` of its own — stdlib.md doesn't define one, only `Map` does.
//! The `T[] initial` list constructor's interaction with `ValueEquatable`
//! is not implemented — `contains`/map key equality fall back to
//! primitive/string value equality or reference identity (see
//! `nl_vm::native`), never `valueEquals`/`valueHash`.
//!
//! `system.MapEntry<K, V>` (stdlib.md § Result types) is covered as a
//! third native generic: never constructed by user code, only returned by
//! `Map.entries()` (and the for-each loop over a map, which desugars
//! through it), with two public fields `key`/`value` typed from the
//! mangled name like everything else here.

use nl_syntax::ast::Type;

/// `"system.List"` or `"system.Map"` if `fqcn` is a mangled instantiation
/// of one of them (e.g. `"system.List<int>"`), else `None`.
fn kind_of(fqcn: &str) -> Option<&'static str> {
    if fqcn.starts_with("system.List<") {
        Some("system.List")
    } else if fqcn.starts_with("system.Map<") {
        Some("system.Map")
    } else {
        None
    }
}

pub fn is_instance(fqcn: &str) -> bool {
    kind_of(fqcn).is_some()
}

/// Depth-aware top-level comma split — a naive `split(", ")` would break on
/// `"system.Map<string, int>"` nested inside another mangled name's own
/// argument list.
fn split_top_level(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut start = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(s[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(s[start..].trim());
    parts
}

fn parse_type_segment(s: &str) -> Type {
    if let Some(inner) = s.strip_suffix("[]") {
        return Type::Array(Box::new(parse_type_segment(inner)));
    }
    match s {
        "int" => Type::Int,
        "float" => Type::Float,
        "bool" => Type::Bool,
        "byte" => Type::Byte,
        "string" => Type::StringT,
        "void" => Type::Void,
        "null" => Type::NullT,
        other => Type::Named(other.to_string()),
    }
}

/// The concrete type argument(s) of a mangled instantiation, e.g.
/// `"system.Map<string, int>"` -> `[StringT, Int]`. Panics if `fqcn` isn't
/// a mangled generic name (callers only reach here after `kind_of` matched).
fn type_args(fqcn: &str) -> Vec<Type> {
    let start = fqcn.find('<').expect("type_args called on non-generic fqcn") + 1;
    let inner = &fqcn[start..fqcn.len() - 1];
    split_top_level(inner).into_iter().map(parse_type_segment).collect()
}

/// `(param_types, return_type)` for `fqcn.name(argc args)`, or `None` if
/// unknown — same shape as `stdlib::lookup`, but derived from `fqcn`'s own
/// mangled type argument(s) instead of a fixed table.
pub fn method_signature(fqcn: &str, name: &str, argc: usize) -> Option<(Vec<Type>, Type)> {
    let kind = kind_of(fqcn)?;
    let args = type_args(fqcn);
    match kind {
        "system.List" => {
            let t = args.first()?.clone();
            match (name, argc) {
                ("size", 0) => Some((vec![], Type::Int)),
                ("get", 1) => Some((vec![Type::Int], t)),
                ("set", 2) => Some((vec![Type::Int, t.clone()], Type::Void)),
                ("pushBack", 1) | ("pushFront", 1) | ("add", 1) => Some((vec![t.clone()], Type::Void)),
                ("popBack", 0) | ("popFront", 0) => Some((vec![], t)),
                ("remove", 1) => Some((vec![Type::Int], t)),
                ("contains", 1) => Some((vec![t.clone()], Type::Bool)),
                _ => None,
            }
        }
        "system.Map" => {
            let k = args.first()?.clone();
            let v = args.get(1)?.clone();
            match (name, argc) {
                ("size", 0) => Some((vec![], Type::Int)),
                ("get", 1) => Some((vec![k.clone()], Type::Union(vec![v.clone(), Type::NullT]))),
                ("set", 2) => Some((vec![k.clone(), v.clone()], Type::Void)),
                ("remove", 1) => Some((vec![k.clone()], Type::Bool)),
                ("has", 1) => Some((vec![k.clone()], Type::Bool)),
                ("keys", 0) => Some((vec![], Type::Array(Box::new(k.clone())))),
                ("values", 0) => Some((vec![], Type::Array(Box::new(v.clone())))),
                ("entries", 0) => Some((vec![], Type::Array(Box::new(Type::Named(entry_fqcn_of_map(fqcn)))))),
                // `map.forEach((K key, V value) => void f)` — the callback
                // parameter has no real static type either (same
                // `Type::Void` joker every closure checks as, see
                // `checker.rs`'s `Expr::Closure` arm), so its declared
                // shape isn't validated against `K`/`V` here.
                ("forEach", 1) => Some((vec![Type::Void], Type::Void)),
                _ => None,
            }
        }
        _ => None,
    }
}

/// The mangled `MapEntry` instantiation matching a mangled `Map`
/// instantiation — same type-argument list verbatim, so
/// `"system.Map<string, int>"` -> `"system.MapEntry<string, int>"`.
fn entry_fqcn_of_map(map_fqcn: &str) -> String {
    format!("system.MapEntry<{}", &map_fqcn["system.Map<".len()..])
}

/// Public fields of a native generic result type — only
/// `system.MapEntry<K, V>` has any (stdlib.md § system.MapEntry).
pub fn field_ty(fqcn: &str, name: &str) -> Option<Type> {
    if !fqcn.starts_with("system.MapEntry<") {
        return None;
    }
    let args = type_args(fqcn);
    match name {
        "key" => args.first().cloned(),
        "value" => args.get(1).cloned(),
        _ => None,
    }
}

/// Element type seen by a for-each loop over a native generic collection
/// (vm.md § For-each loops): `T` for `system.List<T>`, `MapEntry<K, V>`
/// for `system.Map<K, V>` (iteration goes through `entries()`).
pub fn foreach_element_ty(fqcn: &str) -> Option<Type> {
    match kind_of(fqcn)? {
        "system.List" => type_args(fqcn).first().cloned(),
        "system.Map" => Some(Type::Named(entry_fqcn_of_map(fqcn))),
        _ => None,
    }
}
