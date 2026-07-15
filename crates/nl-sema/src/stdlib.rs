//! Signatures for the native `system.*` classes — stdlib.md § system.Out,
//! system.Err, system.In, system.Int, system.Float, system.Bool, system.String.
//! These classes have no `.nl` source (the VM intercepts calls to them directly,
//! see nl_vm::native), so nl-sema can't discover their signatures from a
//! parsed `SourceFile` the way it does for user classes; this table is the
//! equivalent hand-written source of truth, mirrored by
//! `nl_codegen::stdlib` (kept independent, matching this crate's existing
//! two-copies-of-class_table pattern rather than a shared dependency).
//!
//! Only part of stdlib.md is covered so far (PLAN.md Phase 6): output,
//! int/float/bool parsing/formatting, and system.String. File I/O, List/Map,
//! threads, etc. are future work.

use nl_syntax::ast::Type;

/// `(param_types, return_type)` for `fqcn.name(argc args)`, or `None` if
/// unknown (falls back to the caller's existing lenient handling).
///
/// `system.Out`/`system.Err`'s `print`/`println` accept any of
/// `int|float|bool|string` — encoded as a union so the caller's ordinary
/// union-member assignability check (`is_assignable`) accepts all four
/// without a special case, matching the runtime's to-string normalization
/// (stdlib.md: "behave as if the value were converted to its string
/// representation first").
/// `system.String` entries are keyed by the *total* argument count
/// including the receiver — see `nl_codegen::stdlib::signature`'s matching
/// comment: `text.trim()` and `system.String.trim(text)` are equivalent
/// (stdlib.md), and `checker.rs`'s `Type::StringT` arm prepends the
/// receiver's type before looking up here, same as the static-call path
/// just above it.
pub fn lookup(fqcn: &str, name: &str, argc: usize) -> Option<(Vec<Type>, Type)> {
    let printable = Type::Union(vec![Type::StringT, Type::Int, Type::Float, Type::Bool]);
    let nullable = |t: Type| Type::Union(vec![t, Type::NullT]);
    let string_array = Type::Array(Box::new(Type::StringT));
    match (fqcn, name, argc) {
        ("system.Out", "print", 1) | ("system.Out", "println", 1) => Some((vec![printable], Type::Void)),
        ("system.Err", "print", 1) | ("system.Err", "println", 1) => Some((vec![printable], Type::Void)),
        ("system.In", "readLine", 0) => Some((vec![], nullable(Type::StringT))),
        ("system.Int", "parse", 1) => Some((vec![Type::StringT], Type::Int)),
        ("system.Int", "tryParse", 1) => Some((vec![Type::StringT], nullable(Type::Int))),
        ("system.Int", "toString", 1) => Some((vec![Type::Int], Type::StringT)),
        ("system.Float", "parse", 1) => Some((vec![Type::StringT], Type::Float)),
        ("system.Float", "tryParse", 1) => Some((vec![Type::StringT], nullable(Type::Float))),
        ("system.Float", "toString", 1) => Some((vec![Type::Float], Type::StringT)),
        ("system.Bool", "parse", 1) => Some((vec![Type::StringT], Type::Bool)),
        ("system.Bool", "tryParse", 1) => Some((vec![Type::StringT], nullable(Type::Bool))),
        ("system.Bool", "toString", 1) => Some((vec![Type::Bool], Type::StringT)),
        ("system.String", "length", 1) => Some((vec![Type::StringT], Type::Int)),
        ("system.String", "charAt", 2) => Some((vec![Type::StringT, Type::Int], Type::StringT)),
        ("system.String", "substring", 2) => Some((vec![Type::StringT, Type::Int], Type::StringT)),
        ("system.String", "substring", 3) => Some((vec![Type::StringT, Type::Int, Type::Int], Type::StringT)),
        ("system.String", "indexOf", 2) => Some((vec![Type::StringT, Type::StringT], Type::Int)),
        ("system.String", "indexOf", 3) => Some((vec![Type::StringT, Type::StringT, Type::Int], Type::Int)),
        ("system.String", "contains", 2) => Some((vec![Type::StringT, Type::StringT], Type::Bool)),
        ("system.String", "toUpperCase", 1) => Some((vec![Type::StringT], Type::StringT)),
        ("system.String", "toLowerCase", 1) => Some((vec![Type::StringT], Type::StringT)),
        ("system.String", "replace", 3) => Some((vec![Type::StringT, Type::StringT, Type::StringT], Type::StringT)),
        ("system.String", "startsWith", 2) => Some((vec![Type::StringT, Type::StringT], Type::Bool)),
        ("system.String", "endsWith", 2) => Some((vec![Type::StringT, Type::StringT], Type::Bool)),
        ("system.String", "trim", 1) => Some((vec![Type::StringT], Type::StringT)),
        ("system.String", "split", 2) => Some((vec![Type::StringT, Type::StringT], string_array)),
        _ => None,
    }
}
