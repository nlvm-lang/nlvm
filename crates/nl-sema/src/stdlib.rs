//! Signatures for the native `system.*` classes — stdlib.md § system.Out,
//! system.Err, system.In, system.Int, system.Float, system.Bool,
//! system.String, system.io.*.
//! These classes have no `.nl` source (the VM intercepts calls to them directly,
//! see nl_vm::native), so nl-sema can't discover their signatures from a
//! parsed `SourceFile` the way it does for user classes; this table is the
//! equivalent hand-written source of truth, mirrored by
//! `nl_codegen::stdlib` (kept independent, matching this crate's existing
//! two-copies-of-class_table pattern rather than a shared dependency).
//!
//! Only part of stdlib.md is covered so far (PLAN.md Phase 6): output,
//! int/float/bool parsing/formatting, system.String, file I/O
//! (`system.io.File`/`FileHandle`/`Directory`/`Path`, including `FileMode`
//! and `glob` — see below), and `system.Random`/`SecureRandom`/`Uuid`.
//! Network, threads, etc. are future work.
//!
//! ## `system.io.FileMode`
//!
//! Real enums aren't a language feature yet (PLAN.md still lists them as
//! out of scope), so `FileMode` isn't a genuine user-visible enum — it's
//! modeled as int-constant "fields" on a fake stdlib class, resolved by
//! `enum_const_ty` from a `system.io.FileMode.Read`-shaped dotted
//! `Expr::FieldAccess` chain (mirrors how `lookup`'s callers special-case a
//! dotted `Expr::MethodCall` chain for `system.Out.print(...)`). The value
//! type is just `Type::Named("system.io.FileMode")`, which flows through
//! `check_assignable` for free: `types::atom_eq` compares `Type::Named` by
//! name only, so `File.open`'s declared second parameter type matches
//! without needing a `ClassInfo` entry in the class table.

use nl_syntax::ast::Type;

fn file_handle() -> Type {
    Type::Named("system.io.FileHandle".to_string())
}

fn file_mode() -> Type {
    Type::Named("system.io.FileMode".to_string())
}

/// `system.io.FileMode.<name>` — `None` if `fqcn` isn't `"system.io.FileMode"`
/// or `name` isn't one of the six modes stdlib.md documents. See this
/// module's doc comment.
pub fn enum_const_ty(fqcn: &str, name: &str) -> Option<Type> {
    if fqcn == "system.io.FileMode" && FILE_MODES.contains(&name) {
        Some(file_mode())
    } else {
        None
    }
}

/// Shared with `nl_codegen::stdlib::enum_const_value`, which assigns the
/// matching int tag by position in this same list — keep both in sync.
pub const FILE_MODES: [&str; 6] = ["Read", "Write", "Append", "ReadWrite", "ReadWriteTruncate", "ReadWriteAppend"];

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
    let byte_array = Type::Array(Box::new(Type::Byte));
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
        ("system.io.File", "exists", 1) => Some((vec![Type::StringT], Type::Bool)),
        ("system.io.File", "open", 1) => Some((vec![Type::StringT], file_handle())),
        ("system.io.File", "open", 2) => Some((vec![Type::StringT, file_mode()], file_handle())),
        ("system.io.File", "readAllText", 1) => Some((vec![Type::StringT], Type::StringT)),
        ("system.io.File", "writeAllText", 2) => Some((vec![Type::StringT, Type::StringT], Type::Void)),
        ("system.io.File", "glob", 2) => Some((vec![Type::StringT, Type::StringT], string_array)),
        ("system.io.Directory", "list", 1) => Some((vec![Type::StringT], string_array)),
        ("system.io.Directory", "create", 1) => Some((vec![Type::StringT], Type::Void)),
        ("system.io.Directory", "remove", 1) => Some((vec![Type::StringT], Type::Void)),
        ("system.io.Directory", "exists", 1) => Some((vec![Type::StringT], Type::Bool)),
        ("system.io.Path", "join", 1) => Some((vec![string_array], Type::StringT)),
        ("system.io.Path", "dirname", 1) => Some((vec![Type::StringT], Type::StringT)),
        ("system.io.Path", "basename", 1) => Some((vec![Type::StringT], Type::StringT)),
        ("system.io.Path", "extension", 1) => Some((vec![Type::StringT], nullable(Type::StringT))),
        ("system.io.Path", "normalize", 1) => Some((vec![Type::StringT], Type::StringT)),
        ("system.SecureRandom", "nextBytes", 1) => Some((vec![byte_array], Type::Void)),
        ("system.SecureRandom", "nextInt", 0) => Some((vec![], Type::Int)),
        ("system.SecureRandom", "nextInt", 1) => Some((vec![Type::Int], Type::Int)),
        ("system.Uuid", "random", 0) => Some((vec![], Type::StringT)),
        _ => None,
    }
}

/// The one native class whose *instances* the user manipulates
/// (`system.io.File.open` returns one) — unlike the static-only utility
/// classes in `lookup`, its methods dispatch through `INVOKE_INSTANCE` on
/// the receiver's runtime class (see `nl_vm::native`).
pub fn is_native_instance(fqcn: &str) -> bool {
    matches!(fqcn, "system.io.FileHandle" | "system.Random")
}

/// Instance-method signatures for `is_native_instance` classes, keyed by
/// declared argument count. The receiver is *not* a first argument here,
/// unlike `system.String`'s entries in `lookup` — a `FileHandle` really is
/// an object value with instance dispatch, not a static-call rewrite.
pub fn instance_lookup(fqcn: &str, name: &str, argc: usize) -> Option<(Vec<Type>, Type)> {
    let nullable = |t: Type| Type::Union(vec![t, Type::NullT]);
    let byte_array = Type::Array(Box::new(Type::Byte));
    match (fqcn, name, argc) {
        ("system.io.FileHandle", "close", 0) => Some((vec![], Type::Void)),
        ("system.io.FileHandle", "read", 3) => Some((vec![byte_array, Type::Int, Type::Int], Type::Int)),
        ("system.io.FileHandle", "readLine", 0) => Some((vec![], nullable(Type::StringT))),
        ("system.io.FileHandle", "write", 1) => Some((vec![Type::StringT], Type::Void)),
        ("system.io.FileHandle", "write", 3) => Some((vec![byte_array, Type::Int, Type::Int], Type::Void)),
        ("system.io.FileHandle", "flush", 0) => Some((vec![], Type::Void)),
        ("system.Random", "nextInt", 0) => Some((vec![], Type::Int)),
        ("system.Random", "nextInt", 1) => Some((vec![Type::Int], Type::Int)),
        ("system.Random", "nextFloat", 0) => Some((vec![], Type::Float)),
        _ => None,
    }
}

/// Checked exceptions declared by stdlib methods (static and instance
/// forms alike) — stdlib.md's `throws` clauses. Only *checked* types
/// matter here (they feed `require_handled`/E015); runtime exceptions like
/// `NumberFormatException` are exempt from E015 and therefore omitted.
/// Names are prelude FQCNs (see `nl_syntax::prelude`), already resolved.
pub fn throws(fqcn: &str, name: &str) -> &'static [&'static str] {
    match (fqcn, name) {
        ("system.io.File", "open") => &["FileNotFoundException"],
        ("system.io.File", "readAllText") => &["FileNotFoundException", "IOException"],
        ("system.io.File", "writeAllText") => &["IOException"],
        ("system.io.File", "glob") => &["IOException"],
        ("system.io.Directory", "list" | "create" | "remove") => &["IOException"],
        ("system.io.FileHandle", "read" | "readLine" | "write" | "flush") => &["IOException"],
        _ => &[],
    }
}
