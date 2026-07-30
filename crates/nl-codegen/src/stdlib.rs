//! Native `system.*` class signatures — mirrors `nl_sema::stdlib` (kept
//! independent, matching this crate's existing pattern of not sharing
//! `class_table` with nl-sema either). See stdlib.md and vm.md § Standard
//! library binding: these classes have no `.nl` source and no backing
//! bytecode `Module` — the VM intercepts `INVOKE_STATIC` against them
//! directly (`nl_vm::native`), so nl-codegen only needs to emit a
//! `MethodRef` naming them, never a real class file.

use nl_syntax::ast::Type;

pub fn is_stdlib_class(fqcn: &str) -> bool {
    matches!(
        fqcn,
        "system.Out"
            | "system.Err"
            | "system.In"
            | "system.Int"
            | "system.Float"
            | "system.Bool"
            | "system.String"
            | "system.io.File"
            | "system.io.Directory"
            | "system.io.Path"
            | "system.io.Grep"
            | "system.SecureRandom"
            | "system.Uuid"
            | "system.Env"
            | "system.net.TcpStream"
            | "system.net.Http"
            | "system.thread.Thread"
            | "system.ps.Process"
            | "system.text.Regex"
            | "system.text.Encoding"
            | "system.time.DateTime"
            | "system.time.TimeZone"
            | "system.text.json.Json"
            | "system.db.sqlite.Sqlite"
            | "system.db.mysql.Mysql"
    )
}

/// stdlib.md § system.db — mirrors `nl_sema::stdlib`'s copy of these FQCNs.
pub const DB_CONNECTION: &str = "system.db.Connection";
pub const DB_STATEMENT: &str = "system.db.PreparedStatement";
pub const DB_RESULT_SET: &str = "system.db.ResultSet";
pub const DB_ROW: &str = "system.db.Row";
pub const DB_COLUMN_TYPE: &str = "system.db.ColumnType";
pub const DB_OPEN_MODE: &str = "system.db.sqlite.SqliteOpenMode";

fn db(fqcn: &str) -> Type {
    Type::Named(fqcn.to_string())
}

/// Instance methods of the four `system.db` types — mirrors
/// `nl_sema::stdlib::db_instance_lookup`, except for `system.db.Row`'s
/// typed accessors: sema declares their parameter as the union `int|string`
/// (index form / column-name form, same arity), which is exactly what
/// `expr_ty_of` collapses to its first member, so a `getInt("id")` call
/// would be rejected as `string` vs `int`. `key_is_name` carries the call
/// site's answer instead — see `Emitter::compile_method_call`.
fn db_instance_signature(
    fqcn: &str,
    name: &str,
    argc: usize,
    key_is_name: bool,
) -> Option<(Vec<Type>, Type)> {
    let nullable = |t: Type| Type::Union(vec![t, Type::NullT]);
    let byte_array = Type::Array(Box::new(Type::Byte));
    let column_key = if key_is_name {
        Type::StringT
    } else {
        Type::Int
    };
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

/// `system.db.ResultSet` is the one for-each iterable whose loop is driven
/// by `next()`-until-`null` rather than an index (stdlib.md §
/// system.db.ResultSet) — see `Emitter::compile_foreach`. Mirrors
/// `nl_sema::stdlib::foreach_element_ty`.
pub fn is_result_set(fqcn: &str) -> bool {
    fqcn == DB_RESULT_SET
}

/// Whether `fqcn.name(...)`'s single argument is `system.db.Row`'s
/// column-*name* key rather than a column index — see
/// `db_instance_signature`.
pub fn is_row_accessor(fqcn: &str, name: &str) -> bool {
    fqcn == DB_ROW
        && matches!(
            name,
            "columnType" | "isNull" | "getInt" | "getFloat" | "getBool" | "getString" | "getBytes"
        )
}

/// stdlib.md § system.text.json — mirrors `nl_sema::stdlib`'s copy of these
/// FQCNs and of `is_json_value_class`.
pub const JSON_VALUE: &str = "system.text.json.JsonValue";
pub const JSON_NULL: &str = "system.text.json.JsonNull";
pub const JSON_BOOL: &str = "system.text.json.JsonBool";
pub const JSON_NUMBER: &str = "system.text.json.JsonNumber";
pub const JSON_STRING: &str = "system.text.json.JsonString";
pub const JSON_ARRAY: &str = "system.text.json.JsonArray";
pub const JSON_OBJECT: &str = "system.text.json.JsonObject";

pub fn is_json_value_class(fqcn: &str) -> bool {
    matches!(
        fqcn,
        JSON_VALUE | JSON_NULL | JSON_BOOL | JSON_NUMBER | JSON_STRING | JSON_ARRAY | JSON_OBJECT
    )
}

fn json(fqcn: &str) -> Type {
    Type::Named(fqcn.to_string())
}

/// Instance methods of the `JsonValue` family — mirrors
/// `nl_sema::stdlib::json_instance_lookup` (base methods first, whatever
/// the receiver's static class, then the container-specific ones).
fn json_instance_signature(fqcn: &str, name: &str, argc: usize) -> Option<(Vec<Type>, Type)> {
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
        (JSON_OBJECT, "get", 1) => Some((vec![Type::StringT], nullable(json(JSON_VALUE)))),
        (JSON_OBJECT, "set", 2) => Some((vec![Type::StringT, json(JSON_VALUE)], Type::Void)),
        (JSON_OBJECT, "has", 1) => Some((vec![Type::StringT], Type::Bool)),
        (JSON_OBJECT, "remove", 1) => Some((vec![Type::StringT], Type::Bool)),
        (JSON_OBJECT, "keys", 0) => Some((vec![], Type::Array(Box::new(Type::StringT)))),
        (JSON_OBJECT, "entries", 0) => Some((
            vec![],
            Type::Array(Box::new(Type::Named(format!(
                "system.MapEntry<string, {JSON_VALUE}>"
            )))),
        )),
        _ => None,
    }
}

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

/// `pub(crate)`, unlike this module's other type helpers, since
/// `Emitter::compile_stdlib_call`'s `system.ps.Process.run` special case
/// (see this file's `signature` doc comment) needs it directly rather than
/// through this table.
pub(crate) fn process_result() -> Type {
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

/// `system.io.FileMode.<name>` int constant, or `None` if unknown — mirrors
/// `nl_sema::stdlib::enum_const_ty`/`FILE_MODES` (same list, same order;
/// the position *is* the runtime tag `nl_vm::native`'s `File.open` switches
/// on). See that module's doc comment for why this is a constant rather
/// than a real enum.
pub fn enum_const_value(fqcn: &str, name: &str) -> Option<i64> {
    let cases: &[&str] = match fqcn {
        "system.io.FileMode" => &[
            "Read",
            "Write",
            "Append",
            "ReadWrite",
            "ReadWriteTruncate",
            "ReadWriteAppend",
        ],
        // stdlib.md § system.db.ColumnType / § system.db.sqlite.SqliteOpenMode
        // — same int-constant modeling as `FileMode`; mirrors
        // `nl_sema::stdlib::COLUMN_TYPES`/`SQLITE_OPEN_MODES`.
        DB_COLUMN_TYPE => &["Integer", "Float", "Text", "Blob", "Bool", "Null"],
        DB_OPEN_MODE => &["ReadOnly", "ReadWrite", "ReadWriteCreate"],
        _ => return None,
    };
    cases.iter().position(|&m| m == name).map(|i| i as i64)
}

/// The one native class whose *instances* the user manipulates
/// (`system.io.File.open` returns one): its methods compile to an ordinary
/// `INVOKE_INSTANCE` (the VM intercepts by the receiver's runtime class,
/// `nl_vm::native::dispatch_native_instance`), with this table standing in
/// for the `ClassInfo` a bytecode-backed class would provide.
///
/// `key_is_name` is only consulted for `system.db.Row`'s typed accessors
/// (see `db_instance_signature`); every other class ignores it.
pub fn instance_signature(
    fqcn: &str,
    name: &str,
    argc: usize,
    key_is_name: bool,
) -> Option<(Vec<Type>, Type)> {
    let nullable = |t: Type| Type::Union(vec![t, Type::NullT]);
    let byte_array = Type::Array(Box::new(Type::Byte));
    if is_json_value_class(fqcn) {
        return json_instance_signature(fqcn, name, argc);
    }
    if let Some(sig) = db_instance_signature(fqcn, name, argc, key_is_name) {
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

/// Constructor parameter types for native instance classes constructible
/// via `new` directly (unlike `system.io.FileHandle`, only ever produced by
/// `File.open`) — consulted by `Emitter::compile_new` before falling back
/// to `class_table::find_ctor_overload`, same precedence as
/// `native_generics::ctor_param_types`.
pub fn ctor_param_types(fqcn: &str, argc: usize) -> Option<Vec<Type>> {
    match (fqcn, argc) {
        ("system.Random", 0) => Some(vec![]),
        ("system.Random", 1) => Some(vec![Type::Int]),
        ("system.net.TcpListener", 2) => Some(vec![Type::StringT, Type::Int]),
        ("system.net.UdpSocket", 0) => Some(vec![]),
        // `Thread(() => void task)` — `Type::Void` is the same "no real
        // function type this phase" joker `Expr::Closure`'s own synthetic
        // type resolves to elsewhere (see `Emitter::coerce_value`'s
        // matching `ExprTy::Closure` branch, needed here for the first
        // call site that ever passes a closure as a call argument).
        ("system.thread.Thread", 1) => Some(vec![Type::Void]),
        ("system.thread.Mutex", 0) => Some(vec![]),
        ("system.thread.Semaphore", 1) => Some(vec![Type::Int]),
        // stdlib.md § system.text.json — the whole `JsonValue` family is
        // built with `new` (that is the documented way to compose a
        // document); only the three scalar wrappers take an argument.
        (JSON_NULL | JSON_ARRAY | JSON_OBJECT, 0) => Some(vec![]),
        (JSON_BOOL, 1) => Some(vec![Type::Bool]),
        (JSON_NUMBER, 1) => Some(vec![Type::Float]),
        (JSON_STRING, 1) => Some(vec![Type::StringT]),
        _ => None,
    }
}

/// `print`/`println` accept any of `int|float|bool|string` (stdlib.md:
/// "behave as if the value were converted to its string representation
/// first"). Rather than encode that as a union descriptor, nl-codegen
/// normalizes the argument with `ToString` when it isn't already a string
/// and always calls the single native `(string) -> void` overload — see
/// `Emitter::compile_stdlib_call`.
pub fn is_printlike(fqcn: &str, name: &str) -> bool {
    matches!(
        (fqcn, name),
        ("system.Out", "print")
            | ("system.Out", "println")
            | ("system.Err", "print")
            | ("system.Err", "println")
    )
}

/// `(param_types, return_type)` for every other stdlib method — used to
/// build both the call-site argument coercion and the native `MethodRef`'s
/// descriptor.
///
/// `system.String` entries are keyed by the *total* argument count
/// including the receiver, since `text.trim()` (instance form,
/// `Emitter::compile_method_call`) and `system.String.trim(text)` (static
/// form, this function's normal caller `compile_stdlib_call`) both end up
/// emitting the exact same `INVOKE_STATIC system.String.trim(string)`
/// against `system.String` — stdlib.md documents them as equivalent. The
/// instance-call site prepends the already-compiled receiver's type before
/// looking up here, so both call shapes share this single table.
pub fn signature(fqcn: &str, name: &str, argc: usize) -> Option<(Vec<Type>, Type)> {
    let nullable = |t: Type| Type::Union(vec![t, Type::NullT]);
    let string_array = Type::Array(Box::new(Type::StringT));
    let byte_array = Type::Array(Box::new(Type::Byte));
    match (fqcn, name, argc) {
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
        // stdlib.md § system.io.Grep — mirrors `nl_sema::stdlib::lookup`'s
        // matching entries; no union-type special case needed since the two
        // `search` overloads differ in arity.
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
        // stdlib.md § system.Env — mirrors `nl_sema::stdlib::lookup`'s
        // matching entries.
        ("system.Env", "get", 1) => Some((vec![Type::StringT], nullable(Type::StringT))),
        ("system.Env", "set", 2) => Some((vec![Type::StringT, Type::StringT], Type::Void)),
        ("system.Env", "remove", 1) => Some((vec![Type::StringT], Type::Void)),
        ("system.Env", "list", 0) => Some((vec![], string_array)),
        ("system.net.TcpStream", "connect", 2) => {
            Some((vec![Type::StringT, Type::Int], tcp_stream()))
        }
        ("system.net.Http", "get", 1) => Some((vec![Type::StringT], http_response())),
        ("system.net.Http", "post", 2) => {
            Some((vec![Type::StringT, Type::StringT], http_response()))
        }
        ("system.thread.Thread", "sleep", 1) => Some((vec![Type::Int], Type::Void)),
        // `system.ps.Process.run` is deliberately absent here — its two
        // overloads (`string[] args` vs `string command`) share the same
        // arity, and unlike `system.Out.print`'s union of primitives, the
        // two shapes need genuinely different bytecode (no shared
        // normalization), so `compile_stdlib_call` special-cases it before
        // ever reaching this table. See `nl_sema::stdlib::lookup`'s matching
        // comment for why sema's table *can* just use a union type there.
        ("system.ps.Process", "list", 0) => Some((vec![], Type::Array(Box::new(process_info())))),
        ("system.ps.Process", "list", 1) => {
            Some((vec![Type::Int], Type::Array(Box::new(process_info()))))
        }
        ("system.ps.Process", "pid", 0) => Some((vec![], Type::Int)),
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
        ("system.text.Encoding", "encodeUtf8", 1) => {
            Some((vec![Type::StringT], byte_array.clone()))
        }
        ("system.text.Encoding", "decodeUtf8", 1) => {
            Some((vec![byte_array.clone()], Type::StringT))
        }
        ("system.text.Encoding", "base64Encode", 1) => Some((vec![byte_array], Type::StringT)),
        ("system.text.Encoding", "base64Decode", 1) => {
            Some((vec![Type::StringT], Type::Array(Box::new(Type::Byte))))
        }
        // stdlib.md § system.time.DateTime/TimeZone — mirrors
        // `nl_sema::stdlib::lookup`'s matching entries.
        ("system.time.DateTime", "now", 0) => Some((vec![], date_time())),
        ("system.time.DateTime", "now", 1) => Some((vec![time_zone()], date_time())),
        ("system.time.DateTime", "parse", 1) => Some((vec![Type::StringT], date_time())),
        ("system.time.TimeZone", "getDefault", 0) => Some((vec![], time_zone())),
        ("system.time.TimeZone", "get", 1) => Some((vec![Type::StringT], time_zone())),
        // stdlib.md § system.text.json.Json — mirrors
        // `nl_sema::stdlib::lookup`'s matching entries.
        ("system.text.json.Json", "parse", 1) => Some((vec![Type::StringT], json(JSON_VALUE))),
        ("system.text.json.Json", "tryParse", 1) => {
            Some((vec![Type::StringT], nullable(json(JSON_VALUE))))
        }
        ("system.text.json.Json", "stringify", 1) => Some((vec![json(JSON_VALUE)], Type::StringT)),
        ("system.text.json.Json", "stringify", 2) => {
            Some((vec![json(JSON_VALUE), Type::Int], Type::StringT))
        }
        // stdlib.md § system.db.sqlite / § system.db.mysql — mirrors
        // `nl_sema::stdlib::lookup`'s matching entries.
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

/// `system.net.HttpResponse`'s public fields — mirrors
/// `nl_sema::stdlib::result_field_ty` (same non-generic native result type
/// as `system.MapEntry<K,V>` but without a mangled name to parse types
/// out of, so it gets its own small table instead of going through
/// `native_generics::field_ty`).
pub fn result_field_ty(fqcn: &str, name: &str) -> Option<Type> {
    let nullable = |t: Type| Type::Union(vec![t, Type::NullT]);
    match (fqcn, name) {
        ("system.net.HttpResponse", "statusCode") => Some(Type::Int),
        ("system.net.HttpResponse", "body") => Some(Type::StringT),
        ("system.net.HttpResponse", "headers") => {
            Some(nullable(Type::Array(Box::new(Type::StringT))))
        }
        ("system.ps.ProcessInfo", "pid") => Some(Type::Int),
        ("system.ps.ProcessInfo", "command") => Some(Type::StringT),
        ("system.ps.ProcessInfo", "args") => Some(Type::Array(Box::new(Type::StringT))),
        ("system.ps.ProcessInfo", "user") => Some(nullable(Type::StringT)),
        ("system.ps.ProcessResult", "exitCode") => Some(Type::Int),
        ("system.ps.ProcessResult", "stdout") => Some(Type::StringT),
        ("system.ps.ProcessResult", "stderr") => Some(Type::StringT),
        ("system.text.RegexMatch", "fullMatch") => Some(Type::StringT),
        ("system.text.RegexMatch", "groups") => Some(Type::Array(Box::new(Type::StringT))),
        ("system.io.GrepMatch", "path") => Some(Type::StringT),
        ("system.io.GrepMatch", "lineNumber") => Some(Type::Int),
        ("system.io.GrepMatch", "line") => Some(Type::StringT),
        (JSON_BOOL, "value") => Some(Type::Bool),
        (JSON_NUMBER, "value") => Some(Type::Float),
        (JSON_STRING, "value") => Some(Type::StringT),
        _ => None,
    }
}
