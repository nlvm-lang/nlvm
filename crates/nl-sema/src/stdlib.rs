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
//! and `glob` — see below), `system.Random`/`SecureRandom`/`Uuid`, and
//! `system.Env`.
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

fn tcp_stream() -> Type {
    Type::Named("system.net.TcpStream".to_string())
}

fn http_response() -> Type {
    Type::Named("system.net.HttpResponse".to_string())
}

fn process_info() -> Type {
    Type::Named("system.ps.ProcessInfo".to_string())
}

fn process_result() -> Type {
    Type::Named("system.ps.ProcessResult".to_string())
}

fn regex_match() -> Type {
    Type::Named("system.text.RegexMatch".to_string())
}

fn grep_match() -> Type {
    Type::Named("system.io.GrepMatch".to_string())
}

fn date_time() -> Type {
    Type::Named("system.time.DateTime".to_string())
}

fn time_zone() -> Type {
    Type::Named("system.time.TimeZone".to_string())
}

/// stdlib.md § system.text.json — the FQCNs of the `JsonValue` family. The
/// hierarchy itself (each concrete class `extends` `JsonValue`, which
/// `implements` `Stringable`) lives in `class_table::native_parent`, so
/// assignability, casts and `Stringable` operand checks need no special
/// case here.
pub const JSON_VALUE: &str = "system.text.json.JsonValue";
pub const JSON_NULL: &str = "system.text.json.JsonNull";
pub const JSON_BOOL: &str = "system.text.json.JsonBool";
pub const JSON_NUMBER: &str = "system.text.json.JsonNumber";
pub const JSON_STRING: &str = "system.text.json.JsonString";
pub const JSON_ARRAY: &str = "system.text.json.JsonArray";
pub const JSON_OBJECT: &str = "system.text.json.JsonObject";

fn json(fqcn: &str) -> Type {
    Type::Named(fqcn.to_string())
}

/// stdlib.md § system.db — the four driver-agnostic instance types. Like
/// `system.io.FileHandle` they are never written with `new`: a driver
/// factory (`system.db.sqlite.Sqlite.open`, `system.db.mysql.Mysql.connect`)
/// returns a `Connection`, and each type produces the next one down the
/// chain. `system.db.mysql.MysqlConfig` is *not* here — it is a real
/// prelude class (see `nl_syntax::prelude::mysql_config_class`).
pub const DB_CONNECTION: &str = "system.db.Connection";
pub const DB_STATEMENT: &str = "system.db.PreparedStatement";
pub const DB_RESULT_SET: &str = "system.db.ResultSet";
pub const DB_ROW: &str = "system.db.Row";
/// stdlib.md § system.db.ColumnType — modeled as int constants on a fake
/// stdlib class, exactly like `system.io.FileMode` (see this module's doc
/// comment). Unlike `FileMode` it is also a *return* type (`Row.columnType`),
/// which needs nothing extra: `==` between two `Type::Named` operands is
/// already accepted by the checker and compiles to the generic `CmpEq`,
/// which compares the underlying `Value::Int` tags.
pub const DB_COLUMN_TYPE: &str = "system.db.ColumnType";
/// stdlib.md § system.db.sqlite.SqliteOpenMode — input-only, the exact
/// `system.io.FileMode` shape.
pub const DB_OPEN_MODE: &str = "system.db.sqlite.SqliteOpenMode";

fn db(fqcn: &str) -> Type {
    Type::Named(fqcn.to_string())
}

/// Shared with `nl_codegen::stdlib::enum_const_value`, which assigns the
/// matching int tag by position — keep both lists in sync (the position
/// *is* the runtime tag `nl_vm::db` maps values to).
pub const COLUMN_TYPES: [&str; 6] = ["Integer", "Float", "Text", "Blob", "Bool", "Null"];

/// Same contract as `COLUMN_TYPES`; order matches stdlib.md's
/// `SqliteOpenMode` table.
pub const SQLITE_OPEN_MODES: [&str; 3] = ["ReadOnly", "ReadWrite", "ReadWriteCreate"];

/// Instance methods of the four `system.db` types. Mirrored by
/// `nl_codegen::stdlib::db_instance_signature`.
///
/// `Row`'s typed accessors each have two forms at the *same* arity — by
/// 0-based index and by column name (stdlib.md § Column lookup by name) —
/// so their parameter is declared as the union `int|string`, the same trick
/// `system.ps.Process.run` uses in `lookup` below. nl-codegen can't use a
/// union there (it would collapse to its first member and reject the string
/// form), so it resolves the two by the argument's inferred type instead.
fn db_instance_lookup(fqcn: &str, name: &str, argc: usize) -> Option<(Vec<Type>, Type)> {
    let nullable = |t: Type| Type::Union(vec![t, Type::NullT]);
    let byte_array = Type::Array(Box::new(Type::Byte));
    let column_key = Type::Union(vec![Type::Int, Type::StringT]);
    match (fqcn, name, argc) {
        (DB_CONNECTION, "prepare", 1) => Some((vec![Type::StringT], db(DB_STATEMENT))),
        (DB_CONNECTION, "query", 1) => Some((vec![Type::StringT], db(DB_RESULT_SET))),
        (DB_CONNECTION, "execute", 1) => Some((vec![Type::StringT], Type::Int)),
        (DB_CONNECTION, "beginTransaction", 0) => Some((vec![], Type::Void)),
        (DB_CONNECTION, "commit", 0) => Some((vec![], Type::Void)),
        (DB_CONNECTION, "rollback", 0) => Some((vec![], Type::Void)),
        (DB_CONNECTION, "lastInsertId", 0) => Some((vec![], nullable(Type::Int))),
        (DB_CONNECTION, "isClosed", 0) => Some((vec![], Type::Bool)),
        (DB_CONNECTION, "close", 0) => Some((vec![], Type::Void)),
        (DB_STATEMENT, "parameterCount", 0) => Some((vec![], Type::Int)),
        (DB_STATEMENT, "bindInt", 2) => Some((vec![Type::Int, Type::Int], Type::Void)),
        (DB_STATEMENT, "bindFloat", 2) => Some((vec![Type::Int, Type::Float], Type::Void)),
        (DB_STATEMENT, "bindBool", 2) => Some((vec![Type::Int, Type::Bool], Type::Void)),
        (DB_STATEMENT, "bindString", 2) => Some((vec![Type::Int, Type::StringT], Type::Void)),
        (DB_STATEMENT, "bindBytes", 2) => Some((vec![Type::Int, byte_array], Type::Void)),
        (DB_STATEMENT, "bindNull", 1) => Some((vec![Type::Int], Type::Void)),
        (DB_STATEMENT, "query", 0) => Some((vec![], db(DB_RESULT_SET))),
        (DB_STATEMENT, "execute", 0) => Some((vec![], Type::Int)),
        (DB_STATEMENT, "reset", 0) => Some((vec![], Type::Void)),
        (DB_STATEMENT, "isClosed", 0) => Some((vec![], Type::Bool)),
        (DB_STATEMENT, "close", 0) => Some((vec![], Type::Void)),
        (DB_RESULT_SET, "next", 0) => Some((vec![], nullable(db(DB_ROW)))),
        (DB_RESULT_SET, "columnCount", 0) => Some((vec![], Type::Int)),
        (DB_RESULT_SET, "columnName", 1) => Some((vec![Type::Int], Type::StringT)),
        (DB_RESULT_SET, "isClosed", 0) => Some((vec![], Type::Bool)),
        (DB_RESULT_SET, "close", 0) => Some((vec![], Type::Void)),
        (DB_ROW, "columnCount", 0) => Some((vec![], Type::Int)),
        (DB_ROW, "columnType", 1) => Some((vec![column_key], db(DB_COLUMN_TYPE))),
        (DB_ROW, "isNull", 1) => Some((vec![column_key], Type::Bool)),
        (DB_ROW, "getInt", 1) => Some((vec![column_key], nullable(Type::Int))),
        (DB_ROW, "getFloat", 1) => Some((vec![column_key], nullable(Type::Float))),
        (DB_ROW, "getBool", 1) => Some((vec![column_key], nullable(Type::Bool))),
        (DB_ROW, "getString", 1) => Some((vec![column_key], nullable(Type::StringT))),
        (DB_ROW, "getBytes", 1) => Some((
            vec![column_key],
            nullable(Type::Array(Box::new(Type::Byte))),
        )),
        _ => None,
    }
}

/// stdlib.md § system.db.ResultSet: "supports the for-each loop — `for
/// (const auto row : results)` iterates by repeatedly calling `next()` until
/// `null` is returned". The loop variable is a `Row` (never `null`: the
/// `null` terminates the loop rather than being handed to the body), so
/// `checker`'s `StmtKind::ForEach` arm consults this before falling back to
/// `native_generics::foreach_element_ty`. Mirrored by
/// `nl_codegen::stmt::compile_foreach`, which emits the `next()`-until-null
/// loop shape.
pub fn foreach_element_ty(fqcn: &str) -> Option<Type> {
    (fqcn == DB_RESULT_SET).then(|| db(DB_ROW))
}

/// Whether `fqcn` is one of the seven `system.text.json` value classes
/// (`JsonValue` plus its six concrete subclasses) — everything the family
/// shares in nl-sema is keyed off this.
pub fn is_json_value_class(fqcn: &str) -> bool {
    matches!(
        fqcn,
        JSON_VALUE | JSON_NULL | JSON_BOOL | JSON_NUMBER | JSON_STRING | JSON_ARRAY | JSON_OBJECT
    )
}

/// Instance methods of the `JsonValue` family — the `JsonValue` base
/// methods are available on every one of the six concrete classes
/// (inherited), so they are matched first, whatever the receiver's static
/// class; `JsonArray`/`JsonObject`'s own methods come after. Mirrored by
/// `nl_codegen::stdlib::json_instance_signature`.
fn json_instance_lookup(fqcn: &str, name: &str, argc: usize) -> Option<(Vec<Type>, Type)> {
    let nullable = |t: Type| Type::Union(vec![t, Type::NullT]);
    match (name, argc) {
        ("toString", 0) => return Some((vec![], Type::StringT)),
        ("isNull" | "isBool" | "isNumber" | "isString" | "isArray" | "isObject", 0) => {
            return Some((vec![], Type::Bool))
        }
        ("asBool", 0) => return Some((vec![], Type::Bool)),
        ("asNumber", 0) => return Some((vec![], Type::Float)),
        ("asString", 0) => return Some((vec![], Type::StringT)),
        ("asArray", 0) => return Some((vec![], json(JSON_ARRAY))),
        ("asObject", 0) => return Some((vec![], json(JSON_OBJECT))),
        _ => {}
    }
    match (fqcn, name, argc) {
        (JSON_ARRAY, "length", 0) => Some((vec![], Type::Int)),
        (JSON_ARRAY, "get", 1) => Some((vec![Type::Int], json(JSON_VALUE))),
        (JSON_ARRAY, "set", 2) => Some((vec![Type::Int, json(JSON_VALUE)], Type::Void)),
        (JSON_ARRAY, "add", 1) => Some((vec![json(JSON_VALUE)], Type::Void)),
        (JSON_ARRAY, "values", 0) => Some((vec![], Type::Array(Box::new(json(JSON_VALUE))))),
        (JSON_OBJECT, "size", 0) => Some((vec![], Type::Int)),
        // stdlib.md § Absent key vs. JSON `null`: `null` means *absent*, a
        // key holding JSON `null` yields a `JsonNull` instance.
        (JSON_OBJECT, "get", 1) => Some((vec![Type::StringT], nullable(json(JSON_VALUE)))),
        (JSON_OBJECT, "set", 2) => Some((vec![Type::StringT, json(JSON_VALUE)], Type::Void)),
        (JSON_OBJECT, "has", 1) => Some((vec![Type::StringT], Type::Bool)),
        (JSON_OBJECT, "remove", 1) => Some((vec![Type::StringT], Type::Bool)),
        (JSON_OBJECT, "keys", 0) => Some((vec![], Type::Array(Box::new(Type::StringT)))),
        // `system.MapEntry<string, JsonValue>[]` — the same native generic
        // result type `system.Map.entries()` produces, spelled with the
        // mangled instantiation name `crate::native_generics` parses.
        (JSON_OBJECT, "entries", 0) => Some((
            vec![],
            Type::Array(Box::new(Type::Named(format!(
                "system.MapEntry<string, {JSON_VALUE}>"
            )))),
        )),
        _ => None,
    }
}

/// `system.io.FileMode.<name>` — `None` if `fqcn` isn't one of the three
/// enum-like stdlib classes or `name` isn't one of the cases stdlib.md
/// documents for it. See this module's doc comment.
pub fn enum_const_ty(fqcn: &str, name: &str) -> Option<Type> {
    match fqcn {
        "system.io.FileMode" if FILE_MODES.contains(&name) => Some(file_mode()),
        DB_COLUMN_TYPE if COLUMN_TYPES.contains(&name) => Some(db(DB_COLUMN_TYPE)),
        DB_OPEN_MODE if SQLITE_OPEN_MODES.contains(&name) => Some(db(DB_OPEN_MODE)),
        _ => None,
    }
}

/// Shared with `nl_codegen::stdlib::enum_const_value`, which assigns the
/// matching int tag by position in this same list — keep both in sync.
pub const FILE_MODES: [&str; 6] = [
    "Read",
    "Write",
    "Append",
    "ReadWrite",
    "ReadWriteTruncate",
    "ReadWriteAppend",
];

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
/// Whether `fqcn.name` is `system.Out`/`system.Err`'s `print`/`println` —
/// the one stdlib call whose single parameter accepts more than `lookup`'s
/// declared `printable` union can express: a `Stringable`-implementing
/// class (specs.md § Stringable interface lists `print`/`println` as a
/// third consumer, alongside `+` concatenation and `(string)` cast). Used by
/// `Checker::check_expr`'s `Expr::MethodCall` arm to fall back to
/// `is_concat_operand` (the same Stringable check already used for
/// concat/cast) for a `Type::Named` argument, instead of `check_assignable`
/// against the primitive-only `printable` union.
pub fn is_printlike(fqcn: &str, name: &str) -> bool {
    matches!(
        (fqcn, name),
        ("system.Out", "print")
            | ("system.Out", "println")
            | ("system.Err", "print")
            | ("system.Err", "println")
    )
}

pub fn lookup(fqcn: &str, name: &str, argc: usize) -> Option<(Vec<Type>, Type)> {
    let printable = Type::Union(vec![Type::StringT, Type::Int, Type::Float, Type::Bool]);
    let nullable = |t: Type| Type::Union(vec![t, Type::NullT]);
    let string_array = Type::Array(Box::new(Type::StringT));
    let byte_array = Type::Array(Box::new(Type::Byte));
    match (fqcn, name, argc) {
        ("system.Out", "print", 1) | ("system.Out", "println", 1) => {
            Some((vec![printable], Type::Void))
        }
        ("system.Err", "print", 1) | ("system.Err", "println", 1) => {
            Some((vec![printable], Type::Void))
        }
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
        ("system.String", "substring", 3) => {
            Some((vec![Type::StringT, Type::Int, Type::Int], Type::StringT))
        }
        ("system.String", "indexOf", 2) => Some((vec![Type::StringT, Type::StringT], Type::Int)),
        ("system.String", "indexOf", 3) => {
            Some((vec![Type::StringT, Type::StringT, Type::Int], Type::Int))
        }
        ("system.String", "contains", 2) => Some((vec![Type::StringT, Type::StringT], Type::Bool)),
        ("system.String", "toUpperCase", 1) => Some((vec![Type::StringT], Type::StringT)),
        ("system.String", "toLowerCase", 1) => Some((vec![Type::StringT], Type::StringT)),
        ("system.String", "replace", 3) => Some((
            vec![Type::StringT, Type::StringT, Type::StringT],
            Type::StringT,
        )),
        ("system.String", "startsWith", 2) => {
            Some((vec![Type::StringT, Type::StringT], Type::Bool))
        }
        ("system.String", "endsWith", 2) => Some((vec![Type::StringT, Type::StringT], Type::Bool)),
        ("system.String", "trim", 1) => Some((vec![Type::StringT], Type::StringT)),
        ("system.String", "split", 2) => Some((vec![Type::StringT, Type::StringT], string_array)),
        ("system.io.File", "exists", 1) => Some((vec![Type::StringT], Type::Bool)),
        ("system.io.File", "open", 1) => Some((vec![Type::StringT], file_handle())),
        ("system.io.File", "open", 2) => Some((vec![Type::StringT, file_mode()], file_handle())),
        ("system.io.File", "readAllText", 1) => Some((vec![Type::StringT], Type::StringT)),
        ("system.io.File", "writeAllText", 2) => {
            Some((vec![Type::StringT, Type::StringT], Type::Void))
        }
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
        // stdlib.md § system.io.Grep — the two `search` overloads differ in
        // arity (file path vs dirPath+recursive), so unlike
        // `system.ps.Process.run` above they need no union-type trick.
        ("system.io.Grep", "search", 2) => Some((
            vec![Type::StringT, Type::StringT],
            Type::Array(Box::new(grep_match())),
        )),
        ("system.io.Grep", "search", 3) => Some((
            vec![Type::StringT, Type::StringT, Type::Bool],
            Type::Array(Box::new(grep_match())),
        )),
        ("system.SecureRandom", "nextBytes", 1) => Some((vec![byte_array], Type::Void)),
        ("system.SecureRandom", "nextInt", 0) => Some((vec![], Type::Int)),
        ("system.SecureRandom", "nextInt", 1) => Some((vec![Type::Int], Type::Int)),
        ("system.Uuid", "random", 0) => Some((vec![], Type::StringT)),
        // stdlib.md § system.Env — environment variables of the current
        // process; all methods static, no checked exceptions declared.
        ("system.Env", "get", 1) => Some((vec![Type::StringT], nullable(Type::StringT))),
        ("system.Env", "set", 2) => Some((vec![Type::StringT, Type::StringT], Type::Void)),
        ("system.Env", "remove", 1) => Some((vec![Type::StringT], Type::Void)),
        ("system.Env", "list", 0) => Some((vec![], string_array)),
        // stdlib.md § system.net.TcpStream/Http — `connect`/`get`/`post`
        // are the only *static* network methods (everything else on these
        // classes is instance dispatch, see `instance_lookup`).
        ("system.net.TcpStream", "connect", 2) => {
            Some((vec![Type::StringT, Type::Int], tcp_stream()))
        }
        ("system.net.Http", "get", 1) => Some((vec![Type::StringT], http_response())),
        ("system.net.Http", "post", 2) => {
            Some((vec![Type::StringT, Type::StringT], http_response()))
        }
        // `system.thread.Thread.sleep` is the one *static* method on an
        // otherwise instance-dispatch class (`is_native_instance` below) —
        // same shape as `system.net.TcpStream.connect`.
        ("system.thread.Thread", "sleep", 1) => Some((vec![Type::Int], Type::Void)),
        // stdlib.md § system.ps.Process — `run` has two overloads at the
        // same arity (`string[] args` vs `string command`); this table only
        // keys on arity, so both are folded into one union parameter type,
        // same trick as `print`/`println`'s `printable` above. Unlike
        // `print`, no runtime normalization is needed (the VM inspects the
        // actual argument's value variant, see `nl_vm::native`), and
        // nl-codegen special-cases this one call instead of going through
        // its own generic `signature` table (a union collapses to its first
        // member for codegen purposes, which would wrongly reject the
        // `string[]` overload — see `nl_codegen::expr::compile_stdlib_call`).
        ("system.ps.Process", "run", 1) => Some((
            vec![Type::Union(vec![Type::StringT, string_array.clone()])],
            process_result(),
        )),
        ("system.ps.Process", "list", 0) => Some((vec![], Type::Array(Box::new(process_info())))),
        ("system.ps.Process", "list", 1) => {
            Some((vec![Type::Int], Type::Array(Box::new(process_info()))))
        }
        ("system.ps.Process", "pid", 0) => Some((vec![], Type::Int)),
        // `exit` never actually returns (stdlib.md: "Terminal statement:
        // does not return"); `nl-sema::checker::check_stmt` special-cases
        // it (see the doc comment there) so code after it in the same block
        // isn't required to (re)establish definite assignment, mirroring
        // `throw`/`return`. The `Void` return type here is only reached
        // when `exit(...)` is used as a plain expression statement, which
        // is how every real caller uses it.
        ("system.ps.Process", "exit", 1) => Some((vec![Type::Int], Type::Void)),
        ("system.ps.Process", "getCwd", 0) => Some((vec![], Type::StringT)),
        ("system.ps.Process", "setCwd", 1) => Some((vec![Type::StringT], Type::Void)),
        // stdlib.md § system.text.Regex/system.text.Encoding.
        ("system.text.Regex", "match", 2) => Some((vec![Type::StringT, Type::StringT], Type::Bool)),
        ("system.text.Regex", "matchFirst", 2) => {
            Some((vec![Type::StringT, Type::StringT], nullable(regex_match())))
        }
        ("system.text.Regex", "replace", 3) => Some((
            vec![Type::StringT, Type::StringT, Type::StringT],
            Type::StringT,
        )),
        ("system.text.Regex", "split", 2) => {
            Some((vec![Type::StringT, Type::StringT], string_array))
        }
        ("system.text.Regex", "escape", 1) => Some((vec![Type::StringT], Type::StringT)),
        ("system.text.Encoding", "encodeUtf8", 1) => Some((vec![Type::StringT], byte_array)),
        ("system.text.Encoding", "decodeUtf8", 1) => {
            Some((vec![byte_array.clone()], Type::StringT))
        }
        ("system.text.Encoding", "base64Encode", 1) => Some((vec![byte_array], Type::StringT)),
        ("system.text.Encoding", "base64Decode", 1) => {
            Some((vec![Type::StringT], Type::Array(Box::new(Type::Byte))))
        }
        // stdlib.md § system.time.DateTime/TimeZone — factory/parsing
        // statics; accessors/formatting are instance methods, see
        // `instance_lookup` below.
        ("system.time.DateTime", "now", 0) => Some((vec![], date_time())),
        ("system.time.DateTime", "now", 1) => Some((vec![time_zone()], date_time())),
        ("system.time.DateTime", "parse", 1) => Some((vec![Type::StringT], date_time())),
        ("system.time.TimeZone", "getDefault", 0) => Some((vec![], time_zone())),
        ("system.time.TimeZone", "get", 1) => Some((vec![Type::StringT], time_zone())),
        // stdlib.md § system.text.json.Json — the namespace's only
        // static-only class; everything else in it is instance dispatch on
        // a `JsonValue` (see `instance_lookup`).
        ("system.text.json.Json", "parse", 1) => Some((vec![Type::StringT], json(JSON_VALUE))),
        ("system.text.json.Json", "tryParse", 1) => {
            Some((vec![Type::StringT], nullable(json(JSON_VALUE))))
        }
        ("system.text.json.Json", "stringify", 1) => Some((vec![json(JSON_VALUE)], Type::StringT)),
        ("system.text.json.Json", "stringify", 2) => {
            Some((vec![json(JSON_VALUE), Type::Int], Type::StringT))
        }
        // stdlib.md § system.db.sqlite / § system.db.mysql — the two driver
        // factories, the only static entry points into `system.db`.
        // Everything they hand back (`Connection` and, transitively,
        // `PreparedStatement`/`ResultSet`/`Row`) is instance dispatch, see
        // `db_instance_lookup`.
        ("system.db.sqlite.Sqlite", "open", 1) => Some((vec![Type::StringT], db(DB_CONNECTION))),
        ("system.db.sqlite.Sqlite", "open", 2) => {
            Some((vec![Type::StringT, db(DB_OPEN_MODE)], db(DB_CONNECTION)))
        }
        ("system.db.sqlite.Sqlite", "openMemory", 0) => Some((vec![], db(DB_CONNECTION))),
        ("system.db.mysql.Mysql", "connect", 1) => Some((
            // The *resolved* prelude FQCN: `MysqlConfig` is declared
            // namespace-less like every prelude class, and
            // `system.db.mysql.MysqlConfig` is only an import-map alias for
            // it (`prelude::NAMESPACED_ALIASES`). Spelling the qualified
            // name here would make `new system.db.mysql.MysqlConfig(...)`
            // — whose type resolves to the bare name — fail E004.
            vec![Type::Named("MysqlConfig".to_string())],
            db(DB_CONNECTION),
        )),
        _ => None,
    }
}

/// `system.net.HttpResponse`'s public fields (stdlib.md § Result types) —
/// a native result type like `system.MapEntry<K,V>`, but non-generic, so
/// it doesn't go through `native_generics::field_ty`; only ever produced
/// by `system.net.Http.get`/`post`, never constructed by user code.
pub fn result_field_ty(fqcn: &str, name: &str) -> Option<Type> {
    let nullable = |t: Type| Type::Union(vec![t, Type::NullT]);
    match (fqcn, name) {
        ("system.net.HttpResponse", "statusCode") => Some(Type::Int),
        ("system.net.HttpResponse", "body") => Some(Type::StringT),
        ("system.net.HttpResponse", "headers") => {
            Some(nullable(Type::Array(Box::new(Type::StringT))))
        }
        // stdlib.md § Result types — `system.ps.ProcessInfo` (one entry of
        // `Process.list()`) and `system.ps.ProcessResult` (`Process.run()`'s
        // outcome), same non-generic native result shape as `HttpResponse`.
        ("system.ps.ProcessInfo", "pid") => Some(Type::Int),
        ("system.ps.ProcessInfo", "command") => Some(Type::StringT),
        ("system.ps.ProcessInfo", "args") => Some(Type::Array(Box::new(Type::StringT))),
        ("system.ps.ProcessInfo", "user") => Some(nullable(Type::StringT)),
        ("system.ps.ProcessResult", "exitCode") => Some(Type::Int),
        ("system.ps.ProcessResult", "stdout") => Some(Type::StringT),
        ("system.ps.ProcessResult", "stderr") => Some(Type::StringT),
        // stdlib.md § system.text.Regex — `matchFirst`'s result type, same
        // non-generic native result shape as `HttpResponse`.
        ("system.text.RegexMatch", "fullMatch") => Some(Type::StringT),
        ("system.text.RegexMatch", "groups") => Some(Type::Array(Box::new(Type::StringT))),
        // stdlib.md § system.io.Grep — `search`'s result type, same
        // non-generic native result shape as `HttpResponse`.
        ("system.io.GrepMatch", "path") => Some(Type::StringT),
        ("system.io.GrepMatch", "lineNumber") => Some(Type::Int),
        ("system.io.GrepMatch", "line") => Some(Type::StringT),
        // stdlib.md § system.text.json — the three scalar wrappers expose
        // their payload as a `public` field on top of the `asX()` accessors
        // (`final class readonly`, so it is read-only in practice: nothing
        // outside `crate::stdlib` declares it assignable).
        (JSON_BOOL, "value") => Some(Type::Bool),
        (JSON_NUMBER, "value") => Some(Type::Float),
        (JSON_STRING, "value") => Some(Type::StringT),
        _ => None,
    }
}

/// The one native class whose *instances* the user manipulates
/// (`system.io.File.open` returns one) — unlike the static-only utility
/// classes in `lookup`, its methods dispatch through `INVOKE_INSTANCE` on
/// the receiver's runtime class (see `nl_vm::native`).
pub fn is_native_instance(fqcn: &str) -> bool {
    if is_json_value_class(fqcn) {
        return true;
    }
    matches!(fqcn, DB_CONNECTION | DB_STATEMENT | DB_RESULT_SET | DB_ROW)
        || matches!(
            fqcn,
            "system.io.FileHandle"
                | "system.Random"
                | "system.net.TcpListener"
                | "system.net.TcpStream"
                | "system.net.UdpSocket"
                | "system.thread.Thread"
                | "system.thread.Mutex"
                | "system.thread.Semaphore"
                | "system.time.DateTime"
                | "system.time.TimeZone"
        )
}

/// Instance-method signatures for `is_native_instance` classes, keyed by
/// declared argument count. The receiver is *not* a first argument here,
/// unlike `system.String`'s entries in `lookup` — a `FileHandle` really is
/// an object value with instance dispatch, not a static-call rewrite.
pub fn instance_lookup(fqcn: &str, name: &str, argc: usize) -> Option<(Vec<Type>, Type)> {
    let nullable = |t: Type| Type::Union(vec![t, Type::NullT]);
    let byte_array = Type::Array(Box::new(Type::Byte));
    if is_json_value_class(fqcn) {
        return json_instance_lookup(fqcn, name, argc);
    }
    if let Some(sig) = db_instance_lookup(fqcn, name, argc) {
        return Some(sig);
    }
    match (fqcn, name, argc) {
        ("system.io.FileHandle", "close", 0) => Some((vec![], Type::Void)),
        ("system.io.FileHandle", "read", 3) => {
            Some((vec![byte_array, Type::Int, Type::Int], Type::Int))
        }
        ("system.io.FileHandle", "readLine", 0) => Some((vec![], nullable(Type::StringT))),
        ("system.io.FileHandle", "write", 1) => Some((vec![Type::StringT], Type::Void)),
        ("system.io.FileHandle", "write", 3) => {
            Some((vec![byte_array, Type::Int, Type::Int], Type::Void))
        }
        ("system.io.FileHandle", "flush", 0) => Some((vec![], Type::Void)),
        ("system.Random", "nextInt", 0) => Some((vec![], Type::Int)),
        ("system.Random", "nextInt", 1) => Some((vec![Type::Int], Type::Int)),
        ("system.Random", "nextFloat", 0) => Some((vec![], Type::Float)),
        ("system.net.TcpListener", "accept", 0) => Some((vec![], tcp_stream())),
        ("system.net.TcpListener", "close", 0) => Some((vec![], Type::Void)),
        ("system.net.TcpStream", "read", 3) => {
            Some((vec![byte_array.clone(), Type::Int, Type::Int], Type::Int))
        }
        ("system.net.TcpStream", "write", 3) => {
            Some((vec![byte_array, Type::Int, Type::Int], Type::Void))
        }
        ("system.net.TcpStream", "close", 0) => Some((vec![], Type::Void)),
        ("system.net.UdpSocket", "bind", 2) => Some((vec![Type::StringT, Type::Int], Type::Void)),
        ("system.net.UdpSocket", "send", 3) => Some((
            vec![Type::StringT, Type::Int, Type::Array(Box::new(Type::Byte))],
            Type::Void,
        )),
        ("system.net.UdpSocket", "receive", 1) => {
            Some((vec![Type::Array(Box::new(Type::Byte))], Type::Int))
        }
        ("system.net.UdpSocket", "close", 0) => Some((vec![], Type::Void)),
        ("system.thread.Thread", "start", 0) => Some((vec![], Type::Void)),
        ("system.thread.Thread", "join", 0) => Some((vec![], Type::Void)),
        ("system.thread.Thread", "join", 1) => Some((vec![Type::Int], Type::Bool)),
        ("system.thread.Thread", "isAlive", 0) => Some((vec![], Type::Bool)),
        ("system.thread.Thread", "interrupt", 0) => Some((vec![], Type::Void)),
        ("system.thread.Thread", "isInterrupted", 0) => Some((vec![], Type::Bool)),
        ("system.thread.Mutex", "lock", 0) => Some((vec![], Type::Void)),
        ("system.thread.Mutex", "unlock", 0) => Some((vec![], Type::Void)),
        ("system.thread.Mutex", "tryLock", 0) => Some((vec![], Type::Bool)),
        ("system.thread.Semaphore", "acquire", 0) => Some((vec![], Type::Void)),
        ("system.thread.Semaphore", "release", 0) => Some((vec![], Type::Void)),
        ("system.thread.Semaphore", "tryAcquire", 0) => Some((vec![], Type::Bool)),
        ("system.time.DateTime", "getYear", 0) => Some((vec![], Type::Int)),
        ("system.time.DateTime", "getMonth", 0) => Some((vec![], Type::Int)),
        ("system.time.DateTime", "getDay", 0) => Some((vec![], Type::Int)),
        ("system.time.DateTime", "getHour", 0) => Some((vec![], Type::Int)),
        ("system.time.DateTime", "getMinute", 0) => Some((vec![], Type::Int)),
        ("system.time.DateTime", "getSecond", 0) => Some((vec![], Type::Int)),
        ("system.time.DateTime", "getTimeZone", 0) => Some((vec![], time_zone())),
        ("system.time.DateTime", "withTimeZone", 1) => Some((vec![time_zone()], date_time())),
        ("system.time.DateTime", "toUtc", 0) => Some((vec![], date_time())),
        ("system.time.DateTime", "format", 1) => Some((vec![Type::StringT], Type::StringT)),
        ("system.time.TimeZone", "getId", 0) => Some((vec![], Type::StringT)),
        ("system.time.TimeZone", "getOffsetMinutes", 1) => Some((vec![date_time()], Type::Int)),
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
        ("system.io.Grep", "search") => &["IOException"],
        ("system.io.FileHandle", "read" | "readLine" | "write" | "flush") => &["IOException"],
        ("system.net.TcpListener", "construct" | "accept") => &["IOException"],
        ("system.net.TcpStream", "connect" | "read" | "write") => &["IOException"],
        ("system.net.UdpSocket", "bind" | "send" | "receive") => &["IOException"],
        ("system.net.Http", "get" | "post") => &["IOException"],
        // stdlib.md § system.thread.Thread — raised for real by
        // `Thread.interrupt()`, which sets the target thread's interrupt
        // flag; `join`/`join(int)`/`sleep` are the three points that test it
        // and throw (see `nl_vm::native`'s thread section, § Interruption).
        ("system.thread.Thread", "join") => &["InterruptedException"],
        ("system.thread.Thread", "sleep") => &["InterruptedException"],
        ("system.ps.Process", "run" | "setCwd") => &["IOException"],
        ("system.text.Encoding", "base64Decode") => &["FormatException"],
        ("system.time.DateTime", "parse") => &["FormatException"],
        // stdlib.md § system.text.json.Json — `parse` is the checked half
        // of the pair; `tryParse` returns `null` instead of throwing, which
        // is exactly why it needs no `throws` entry here.
        ("system.text.json.Json", "parse") => &["JsonFormatException"],
        // stdlib.md § system.db — every operation that can reach the driver
        // declares `throws SqlException`. `isClosed`/`close`/
        // `parameterCount`/`columnCount`/`lastInsertId` deliberately don't
        // (stdlib.md's tables leave their `throws` clause off: `close` is
        // idempotent and infallible, the rest only read cached state).
        //
        // `system.db.Row`'s accessors are absent for the same reason: their
        // signatures in stdlib.md § system.db.Row carry no `throws` clause
        // either, even though the column-name overloads are documented as
        // raising `SqlException` for an unknown name. Taking the table at
        // its word is also what makes stdlib.md's own example compile — it
        // calls `row.getInt("id")` inside a `try`/`finally` with no
        // `catch (SqlException)` anywhere, which E015 would reject if these
        // declared a checked exception.
        (DB_CONNECTION, "prepare" | "query" | "execute") => &["SqlException"],
        (DB_CONNECTION, "beginTransaction" | "commit" | "rollback") => &["SqlException"],
        (DB_STATEMENT, "bindInt" | "bindFloat" | "bindBool") => &["SqlException"],
        (DB_STATEMENT, "bindString" | "bindBytes" | "bindNull") => &["SqlException"],
        (DB_STATEMENT, "query" | "execute" | "reset") => &["SqlException"],
        (DB_RESULT_SET, "next" | "columnName") => &["SqlException"],
        ("system.db.sqlite.Sqlite", "open" | "openMemory") => &["SqlException"],
        ("system.db.mysql.Mysql", "connect") => &["SqlException"],
        _ => &[],
    }
}
