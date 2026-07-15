//! Signatures for the native template classes `system.List<T>` and
//! `system.Map<K,V>` — stdlib.md § system.List/system.Map, vm.md §
//! Templates (monomorphization). Mirrored by `nl_sema::native_generics`
//! (kept independent, matching this crate's existing pattern for `system.*`
//! natives — see `stdlib.rs`'s doc comment).
//!
//! Unlike `stdlib.rs`'s flat classes, these are *generic*: by the time
//! nl-codegen sees a reference to one, `nl_syntax::monomorphize` has already
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
//! untested. `entries()`/`forEach` (which need a synthetic `MapEntry<K,V>`
//! class and closures-as-native-callbacks respectively) are not
//! implemented, nor is the `T[] initial` list constructor's interaction
//! with `ValueEquatable` — `contains`/map key equality fall back to
//! primitive/string value equality or reference identity (see
//! `nl_vm::native`), never `valueEquals`/`valueHash`.

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

/// Constructor parameter types for `fqcn`'s `argc`-ary `construct`, or
/// `None` if there's no such overload.
pub fn ctor_param_types(fqcn: &str, argc: usize) -> Option<Vec<Type>> {
    let kind = kind_of(fqcn)?;
    match (kind, argc) {
        ("system.List", 0) => Some(vec![]),
        ("system.List", 1) => {
            let t = type_args(fqcn).into_iter().next()?;
            Some(vec![Type::Array(Box::new(t))])
        }
        ("system.Map", 0) => Some(vec![]),
        _ => None,
    }
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
                _ => None,
            }
        }
        _ => None,
    }
}
