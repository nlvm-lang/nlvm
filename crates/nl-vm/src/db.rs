//! stdlib.md § system.db, § system.db.sqlite, § system.db.mysql — SQL
//! database connectivity: the driver-agnostic `Connection` /
//! `PreparedStatement` / `ResultSet` / `Row` types, the `ColumnType` enum,
//! and the two driver factories (`Sqlite.open`/`openMemory`,
//! `Mysql.connect`).
//!
//! Unlike `crate::mini_regex`/`mini_tz`/`text`, this module does *not*
//! hand-roll its backends: SQLite is `rusqlite` (with the `bundled` feature,
//! so the engine is compiled in rather than picked up from the host) and
//! MySQL is the `mysql` crate over rustls' *ring* provider — the same
//! provider `crate::net_http` uses, which is why the `rustls-tls-ring`
//! feature is selected explicitly (see the workspace `Cargo.toml`). Writing
//! either engine from scratch is not comparable to a small regex matcher.
//!
//! ## Handles and ownership
//!
//! All three closable types are plain `Value::Object`s carrying an integer
//! handle into `Program::db` (`__conn__`, `__stmt__`, `__rs__`), exactly the
//! `system.io.FileHandle`/`__fd__` shape. The Rust-side state lives in
//! [`DbRegistry`], one `Mutex` for the whole subsystem: stdlib.md declares
//! `Connection` and everything derived from it **not thread-safe** ("open one
//! `Connection` per thread or serialize access with `system.thread.Mutex`"),
//! so a single registry lock is never a correctness problem and keeps the
//! parent/child bookkeeping (a `Connection` remembering its statements and
//! result sets) in one place.
//!
//! `close()` is idempotent on all three types and closing a `Connection`
//! also closes every `PreparedStatement` and `ResultSet` derived from it;
//! afterwards every operation other than `close`/`isClosed` throws
//! `SqlException`, per stdlib.md § Resource lifetime.
//!
//! ## Result sets are materialized
//!
//! `ResultSet` reads *all* rows up front and hands them out one at a time
//! from `next()`, rather than streaming from a live driver cursor. rusqlite's
//! `Rows` borrows its `Statement`, which borrows the `Connection`; keeping
//! that alive across NL bytecode execution would need a self-referential
//! handle. Materializing trades memory on large `SELECT`s for a
//! straightforward, `unsafe`-free implementation — the same pragmatic call
//! `system.Map`'s O(n) parallel-array lookup makes.
//!
//! It stays observationally faithful to stdlib.md on the two points that are
//! actually specified: `next()` returns `null` at the end, and a `Row` is
//! invalidated by the following `next()`/`close()`. The latter is enforced
//! explicitly rather than falling out of the representation — a `Row` object
//! carries the cursor position it was handed out at (`__seq__`), and every
//! accessor throws `SqlException` when the result set has moved on.
//!
//! ## Prepared statements
//!
//! A SQLite `PreparedStatement` stores the SQL text and re-prepares through
//! `Connection::prepare_cached` on each `query()`/`execute()` (same borrow
//! problem as above; the cache makes the recompile a hash lookup). The
//! statement is still compiled once at `prepare()` time so a syntax error
//! surfaces there and `parameterCount()` is exact. MySQL's `Statement` is an
//! owned server-side handle, so it is simply kept.
//!
//! Bindings live on the NL side (`Vec<Option<DbValue>>`, 0-based per
//! stdlib.md § Placeholders) and are applied at execution time; `reset()`
//! clears them, and executing with a placeholder left unbound throws
//! `SqlException` as documented.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::error::VmError;
use crate::native::{exception_fields, throw_native};
use crate::program::Program;
use crate::value::{lock, Object, Value};

pub const SQLITE: &str = "system.db.sqlite.Sqlite";
pub const MYSQL: &str = "system.db.mysql.Mysql";
pub const CONNECTION: &str = "system.db.Connection";
pub const STATEMENT: &str = "system.db.PreparedStatement";
pub const RESULT_SET: &str = "system.db.ResultSet";
pub const ROW: &str = "system.db.Row";

const CONN_HANDLE: &str = "__conn__";
const STMT_HANDLE: &str = "__stmt__";
const RS_HANDLE: &str = "__rs__";
/// The cursor position a `Row` was handed out at — see the module doc
/// comment's staleness rule.
const ROW_SEQ: &str = "__seq__";

/// stdlib.md § system.db.ColumnType — the int tags are the positions in
/// `nl_codegen::stdlib::enum_const_value`'s list (`nl_sema::stdlib::
/// COLUMN_TYPES` is the third copy), so `row.columnType(0) ==
/// system.db.ColumnType.Text` compares equal tags.
const TYPE_INTEGER: i64 = 0;
const TYPE_FLOAT: i64 = 1;
const TYPE_TEXT: i64 = 2;
const TYPE_BLOB: i64 = 3;
const TYPE_BOOL: i64 = 4;
const TYPE_NULL: i64 = 5;

/// stdlib.md § system.db.sqlite.SqliteOpenMode — same positional encoding.
const MODE_READ_ONLY: i64 = 0;
const MODE_READ_WRITE: i64 = 1;
const MODE_READ_WRITE_CREATE: i64 = 2;

pub fn is_db_class(fqcn: &str) -> bool {
    matches!(fqcn, SQLITE | MYSQL)
}

pub fn is_db_instance_class(fqcn: &str) -> bool {
    matches!(fqcn, CONNECTION | STATEMENT | RESULT_SET | ROW)
}

// ---------------------------------------------------------------------------
// Values
// ---------------------------------------------------------------------------

/// One column value, normalized across drivers. Deliberately smaller than
/// either driver's own value type: stdlib.md § system.db.ColumnType defines
/// exactly this set, and anything a driver reports outside it (MySQL's
/// `DATE`/`TIME`, for instance) is mapped onto `Text`.
#[derive(Debug, Clone)]
enum DbValue {
    Null,
    Int(i64),
    Float(f64),
    Bool(bool),
    Text(String),
    Blob(Vec<u8>),
}

impl DbValue {
    fn column_type(&self) -> i64 {
        match self {
            DbValue::Null => TYPE_NULL,
            DbValue::Int(_) => TYPE_INTEGER,
            DbValue::Float(_) => TYPE_FLOAT,
            DbValue::Bool(_) => TYPE_BOOL,
            DbValue::Text(_) => TYPE_TEXT,
            DbValue::Blob(_) => TYPE_BLOB,
        }
    }

    fn type_name(&self) -> &'static str {
        match self {
            DbValue::Null => "NULL",
            DbValue::Int(_) => "INTEGER",
            DbValue::Float(_) => "FLOAT",
            DbValue::Bool(_) => "BOOL",
            DbValue::Text(_) => "TEXT",
            DbValue::Blob(_) => "BLOB",
        }
    }
}

/// `InvalidCastException` for a typed accessor that cannot represent the
/// stored value — stdlib.md § system.db.Row ("Throws `InvalidCastException`
/// if the value cannot be represented as ...", a *runtime* exception, so no
/// `throws` clause is involved).
fn bad_cast(value: &DbValue, target: &str) -> VmError {
    throw_native(
        "InvalidCastException",
        format!("cannot read {} column as {target}", value.type_name()),
    )
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

enum Backend {
    Sqlite(Box<rusqlite::Connection>),
    Mysql(Box<mysql::Conn>),
}

struct ConnectionEntry {
    backend: Backend,
    /// Tracked here rather than asked of the driver: stdlib.md requires
    /// `beginTransaction`/`commit`/`rollback` to throw when a transaction is
    /// already active / not active, and neither driver reports that
    /// uniformly.
    in_transaction: bool,
    statements: Vec<i64>,
    result_sets: Vec<i64>,
}

struct StatementEntry {
    conn: i64,
    /// SQLite only re-prepares from the text (see the module doc comment);
    /// kept for MySQL too, purely for error messages.
    sql: String,
    param_count: usize,
    binds: Vec<Option<DbValue>>,
    mysql_stmt: Option<mysql::Statement>,
}

struct ResultSetEntry {
    conn: i64,
    columns: Vec<String>,
    rows: Vec<Vec<DbValue>>,
    /// Number of `next()` calls made so far; the current row is
    /// `rows[cursor - 1]`, and `0` means "before the first row".
    cursor: usize,
}

/// The whole `system.db` Rust-side state — one instance per `Program`, see
/// the module doc comment. Slots are never reused (`close` leaves a `None`
/// hole), so a stale handle always reads as closed rather than silently
/// aliasing a newer object.
#[derive(Default)]
pub struct DbRegistry {
    connections: Vec<Option<ConnectionEntry>>,
    statements: Vec<Option<StatementEntry>>,
    result_sets: Vec<Option<ResultSetEntry>>,
}

impl DbRegistry {
    fn add_connection(&mut self, entry: ConnectionEntry) -> i64 {
        self.connections.push(Some(entry));
        (self.connections.len() - 1) as i64
    }

    fn conn_mut(&mut self, id: i64) -> Option<&mut ConnectionEntry> {
        self.connections.get_mut(id as usize)?.as_mut()
    }

    /// Closing a connection closes everything derived from it (stdlib.md §
    /// Resource lifetime), and implicitly rolls back a pending transaction —
    /// which both drivers already do when the connection object drops.
    fn close_connection(&mut self, id: i64) {
        let Some(slot) = self.connections.get_mut(id as usize) else {
            return;
        };
        let Some(entry) = slot.take() else {
            return;
        };
        for stmt in entry.statements {
            if let Some(s) = self.statements.get_mut(stmt as usize) {
                *s = None;
            }
        }
        for rs in entry.result_sets {
            if let Some(r) = self.result_sets.get_mut(rs as usize) {
                *r = None;
            }
        }
    }

    fn add_statement(&mut self, entry: StatementEntry) -> i64 {
        let conn = entry.conn;
        self.statements.push(Some(entry));
        let id = (self.statements.len() - 1) as i64;
        if let Some(c) = self.conn_mut(conn) {
            c.statements.push(id);
        }
        id
    }

    fn add_result_set(&mut self, entry: ResultSetEntry) -> i64 {
        let conn = entry.conn;
        self.result_sets.push(Some(entry));
        let id = (self.result_sets.len() - 1) as i64;
        if let Some(c) = self.conn_mut(conn) {
            c.result_sets.push(id);
        }
        id
    }

    /// A statement is closed when its own slot is empty *or* its parent
    /// connection has been closed — stdlib.md: "`isClosed` returns `true`
    /// ... or if the parent connection has been closed".
    fn statement_is_closed(&self, id: i64) -> bool {
        match self.statements.get(id as usize).and_then(|s| s.as_ref()) {
            Some(entry) => self
                .connections
                .get(entry.conn as usize)
                .and_then(|c| c.as_ref())
                .is_none(),
            None => true,
        }
    }

    fn result_set_is_closed(&self, id: i64) -> bool {
        match self.result_sets.get(id as usize).and_then(|r| r.as_ref()) {
            Some(entry) => self
                .connections
                .get(entry.conn as usize)
                .and_then(|c| c.as_ref())
                .is_none(),
            None => true,
        }
    }
}

// ---------------------------------------------------------------------------
// Exceptions
// ---------------------------------------------------------------------------

/// stdlib.md § Exception table: `SqlException` "carries `sqlState` (SQLSTATE
/// code, empty string when the driver does not provide one) and `errorCode`
/// (driver-specific numeric code, `0` when not provided)". Built directly
/// like every other VM-raised exception (`native::throw_native`), with those
/// two extra fields on top of `Exception.message`/`stackTrace`.
fn throw_sql(message: impl Into<String>, sql_state: &str, error_code: i64) -> VmError {
    let mut fields = exception_fields(message);
    fields.insert(
        "sqlState".to_string(),
        Value::Str(Arc::new(sql_state.to_string())),
    );
    fields.insert("errorCode".to_string(), Value::Int(error_code));
    VmError::Thrown(Value::Object(Arc::new(Mutex::new(Object::native(
        "SqlException",
        fields,
    )))))
}

/// The common case: a failure the driver did not attach a SQLSTATE/code to
/// (closed handle, out-of-range index, unknown column name, ...).
fn sql_error(message: impl Into<String>) -> VmError {
    throw_sql(message, "", 0)
}

/// rusqlite reports the SQLite extended result code but no SQLSTATE (SQLite
/// has none), so `sqlState` stays empty and `errorCode` carries the code.
fn sqlite_error(context: &str, e: rusqlite::Error) -> VmError {
    let code = match &e {
        rusqlite::Error::SqliteFailure(err, _) => err.extended_code as i64,
        _ => 0,
    };
    throw_sql(format!("{context}: {e}"), "", code)
}

/// MySQL server errors carry both a SQLSTATE and a numeric error code;
/// anything else (I/O, TLS, protocol) has neither.
fn mysql_error(context: &str, e: mysql::Error) -> VmError {
    if let mysql::Error::MySqlError(err) = &e {
        return throw_sql(
            format!("{context}: {}", err.message),
            &err.state,
            err.code as i64,
        );
    }
    throw_sql(format!("{context}: {e}"), "", 0)
}

// ---------------------------------------------------------------------------
// Object construction / handle extraction
// ---------------------------------------------------------------------------

fn handle_object(class_name: &str, field: &str, id: i64) -> Value {
    let mut fields = HashMap::new();
    fields.insert(field.to_string(), Value::Int(id));
    Value::Object(Arc::new(Mutex::new(Object::native(class_name, fields))))
}

fn handle_of(receiver: &Value, field: &str) -> Result<i64, VmError> {
    let Value::Object(obj) = receiver else {
        return Err(VmError::Malformed("expected system.db receiver"));
    };
    match lock(obj).fields.get(field) {
        Some(Value::Int(id)) => Ok(*id),
        _ => Err(VmError::Malformed("malformed system.db handle object")),
    }
}

fn int_arg(args: &[Value], index: usize) -> Result<i64, VmError> {
    match args.get(index) {
        Some(Value::Int(v)) => Ok(*v),
        Some(Value::Byte(b)) => Ok(*b as i64),
        _ => Err(VmError::Malformed("expected int argument to native call")),
    }
}

fn str_arg(args: &[Value], index: usize) -> Result<String, VmError> {
    match args.get(index) {
        Some(Value::Str(s)) => Ok((**s).clone()),
        _ => Err(VmError::Malformed(
            "expected string argument to native call",
        )),
    }
}

fn bool_arg(args: &[Value], index: usize) -> Result<bool, VmError> {
    match args.get(index) {
        Some(Value::Bool(b)) => Ok(*b),
        _ => Err(VmError::Malformed("expected bool argument to native call")),
    }
}

fn bytes_arg(args: &[Value], index: usize) -> Result<Vec<u8>, VmError> {
    let Some(Value::Array(items)) = args.get(index) else {
        return Err(VmError::Malformed(
            "expected byte[] argument to native call",
        ));
    };
    lock(items)
        .iter()
        .map(|v| match v {
            Value::Byte(b) => Ok(*b),
            // Same low-order-bits rule as the `(byte)` cast, matching
            // `FileHandle.write`'s handling of an `int`-filled `byte[]`.
            Value::Int(i) => Ok(*i as u8),
            _ => Err(VmError::Malformed(
                "expected byte[] argument to native call",
            )),
        })
        .collect()
}

fn value_from_db(v: &DbValue) -> Value {
    match v {
        DbValue::Null => Value::Null,
        DbValue::Int(i) => Value::Int(*i),
        DbValue::Float(f) => Value::Float(*f),
        DbValue::Bool(b) => Value::Bool(*b),
        DbValue::Text(s) => Value::Str(Arc::new(s.clone())),
        DbValue::Blob(b) => Value::Array(Arc::new(Mutex::new(
            b.iter().copied().map(Value::Byte).collect(),
        ))),
    }
}

// ---------------------------------------------------------------------------
// Driver factories (INVOKE_STATIC)
// ---------------------------------------------------------------------------

pub fn dispatch_static(
    program: &Arc<Program>,
    fqcn: &str,
    name: &str,
    args: Vec<Value>,
) -> Result<Option<Value>, VmError> {
    match (fqcn, name) {
        (SQLITE, "open") => {
            let path = str_arg(&args, 0)?;
            // stdlib.md § system.db.sqlite.Sqlite: the 1-argument form is
            // `ReadWriteCreate`. The 2-argument form's mode int is the
            // position of the variant name in
            // `nl_codegen::stdlib::enum_const_value`'s list, same encoding
            // as `system.io.File.open`'s `FileMode`.
            let mode = if args.len() > 1 {
                int_arg(&args, 1)?
            } else {
                MODE_READ_WRITE_CREATE
            };
            use rusqlite::OpenFlags;
            let flags = match mode {
                MODE_READ_ONLY => OpenFlags::SQLITE_OPEN_READ_ONLY,
                MODE_READ_WRITE => OpenFlags::SQLITE_OPEN_READ_WRITE,
                MODE_READ_WRITE_CREATE => {
                    OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE
                }
                _ => return Err(VmError::Malformed("invalid SqliteOpenMode value")),
            } | OpenFlags::SQLITE_OPEN_URI
                | OpenFlags::SQLITE_OPEN_NO_MUTEX;
            // stdlib.md § Security (path traversal): no sanitization here,
            // deliberately — same contract as `system.io.File`, and the
            // caller is told to `Path.normalize` untrusted input first.
            let conn = rusqlite::Connection::open_with_flags(&path, flags)
                .map_err(|e| sqlite_error(&format!("open {path}"), e))?;
            Ok(Some(register_connection(
                program,
                Backend::Sqlite(Box::new(conn)),
            )))
        }
        (SQLITE, "openMemory") => {
            let conn = rusqlite::Connection::open_in_memory()
                .map_err(|e| sqlite_error("open in-memory database", e))?;
            Ok(Some(register_connection(
                program,
                Backend::Sqlite(Box::new(conn)),
            )))
        }
        (MYSQL, "connect") => {
            let conn = mysql_connect(args.first())?;
            Ok(Some(register_connection(
                program,
                Backend::Mysql(Box::new(conn)),
            )))
        }
        _ => Err(VmError::MethodNotFound(format!("{fqcn}.{name}"))),
    }
}

fn register_connection(program: &Arc<Program>, backend: Backend) -> Value {
    let id = lock(program.db()).add_connection(ConnectionEntry {
        backend,
        in_transaction: false,
        statements: Vec::new(),
        result_sets: Vec::new(),
    });
    handle_object(CONNECTION, CONN_HANDLE, id)
}

/// stdlib.md § system.db.mysql — reads the six documented fields off a
/// `MysqlConfig` instance. That class is a real prelude class (see
/// `nl_syntax::prelude::mysql_config_class`), so this is a plain field read
/// on an ordinary `Value::Object`, not a native result type.
///
/// TLS: `useTls = true` uses the `mysql` crate's default `SslOpts`, which
/// validates the chain against the platform trust store, checks expiry and
/// verifies the hostname — stdlib.md makes that a MUST and specifies no way
/// to disable it, so no "accept invalid certs" knob is exposed. A validation
/// failure surfaces as an ordinary connection error, i.e. `SqlException`.
fn mysql_connect(config: Option<&Value>) -> Result<mysql::Conn, VmError> {
    let Some(Value::Object(obj)) = config else {
        return Err(VmError::Malformed(
            "expected MysqlConfig argument to native call",
        ));
    };
    let fields = lock(obj).fields.clone();
    let text = |name: &str| match fields.get(name) {
        Some(Value::Str(s)) => Ok((**s).clone()),
        _ => Err(VmError::Malformed("malformed MysqlConfig object")),
    };
    let host = text("host")?;
    let user = text("user")?;
    let password = text("password")?;
    let database = text("database")?;
    let Some(Value::Int(port)) = fields.get("port") else {
        return Err(VmError::Malformed("malformed MysqlConfig object"));
    };
    let Some(Value::Bool(use_tls)) = fields.get("useTls") else {
        return Err(VmError::Malformed("malformed MysqlConfig object"));
    };
    let mut opts = mysql::OptsBuilder::new()
        .ip_or_hostname(Some(host.clone()))
        .tcp_port(*port as u16)
        .user(Some(user))
        .pass(Some(password));
    // stdlib.md: an empty `database` means "no default database", which is
    // `None` rather than `Some("")` (the latter would ask the server to
    // select a schema literally named "").
    if !database.is_empty() {
        opts = opts.db_name(Some(database));
    }
    opts = if *use_tls {
        opts.ssl_opts(Some(mysql::SslOpts::default()))
    } else {
        opts.ssl_opts(None)
    };
    // stdlib.md § Security (credentials): the password is never echoed —
    // only the host is named in the error message.
    mysql::Conn::new(opts).map_err(|e| mysql_error(&format!("connect to {host}"), e))
}

// ---------------------------------------------------------------------------
// Instance dispatch (INVOKE_INSTANCE)
// ---------------------------------------------------------------------------

pub fn dispatch_instance(
    program: &Arc<Program>,
    class_name: &str,
    name: &str,
    receiver: &Value,
    args: Vec<Value>,
) -> Result<Option<Value>, VmError> {
    match class_name {
        CONNECTION => dispatch_connection(program, name, receiver, args),
        STATEMENT => dispatch_statement(program, name, receiver, args),
        RESULT_SET => dispatch_result_set(program, name, receiver, args),
        ROW => dispatch_row(program, name, receiver, args),
        _ => Err(VmError::MethodNotFound(format!("{class_name}.{name}"))),
    }
}

fn dispatch_connection(
    program: &Arc<Program>,
    name: &str,
    receiver: &Value,
    args: Vec<Value>,
) -> Result<Option<Value>, VmError> {
    let id = handle_of(receiver, CONN_HANDLE)?;
    // `close`/`isClosed` are the two operations a closed connection still
    // answers (stdlib.md § Resource lifetime); everything below them throws.
    match name {
        "close" => {
            lock(program.db()).close_connection(id);
            return Ok(None);
        }
        "isClosed" => {
            let closed = lock(program.db()).conn_mut(id).is_none();
            return Ok(Some(Value::Bool(closed)));
        }
        _ => {}
    }
    let mut registry = lock(program.db());
    if registry.conn_mut(id).is_none() {
        return Err(sql_error(format!("{name} on a closed connection")));
    }
    match name {
        "prepare" => {
            let sql = str_arg(&args, 0)?;
            let (param_count, mysql_stmt) = match &mut registry
                .conn_mut(id)
                .expect("connection presence checked above")
                .backend
            {
                Backend::Sqlite(conn) => {
                    // Compiled once here so a syntax error is reported by
                    // `prepare` (stdlib.md) and the placeholder count is
                    // exact; the compiled form is dropped again — see the
                    // module doc comment on re-preparing.
                    let stmt = conn
                        .prepare(&sql)
                        .map_err(|e| sqlite_error(&format!("prepare {sql}"), e))?;
                    (stmt.parameter_count(), None)
                }
                Backend::Mysql(conn) => {
                    let stmt = mysql::prelude::Queryable::prep(conn.as_mut(), &sql)
                        .map_err(|e| mysql_error(&format!("prepare {sql}"), e))?;
                    (stmt.num_params() as usize, Some(stmt))
                }
            };
            let stmt_id = registry.add_statement(StatementEntry {
                conn: id,
                sql,
                param_count,
                binds: vec![None; param_count],
                mysql_stmt,
            });
            Ok(Some(handle_object(STATEMENT, STMT_HANDLE, stmt_id)))
        }
        "query" => {
            let sql = str_arg(&args, 0)?;
            let (columns, rows) = match &mut registry
                .conn_mut(id)
                .expect("connection presence checked above")
                .backend
            {
                Backend::Sqlite(conn) => sqlite_query(conn, &sql, &[])?,
                Backend::Mysql(conn) => mysql_query(conn, &sql, None, &[])?,
            };
            let rs_id = registry.add_result_set(ResultSetEntry {
                conn: id,
                columns,
                rows,
                cursor: 0,
            });
            Ok(Some(handle_object(RESULT_SET, RS_HANDLE, rs_id)))
        }
        "execute" => {
            let sql = str_arg(&args, 0)?;
            let entry = registry
                .conn_mut(id)
                .expect("connection presence checked above");
            let affected = match &mut entry.backend {
                Backend::Sqlite(conn) => sqlite_execute(conn, &sql, &[])?,
                Backend::Mysql(conn) => mysql_execute(conn, &sql, None, &[])?,
            };
            Ok(Some(Value::Int(affected)))
        }
        "beginTransaction" | "commit" | "rollback" => {
            let entry = registry
                .conn_mut(id)
                .expect("connection presence checked above");
            let (sql, wanted_active) = match name {
                "beginTransaction" => ("BEGIN", false),
                "commit" => ("COMMIT", true),
                _ => ("ROLLBACK", true),
            };
            if entry.in_transaction != wanted_active {
                return Err(sql_error(if wanted_active {
                    format!("{name}: no transaction is active")
                } else {
                    "beginTransaction: a transaction is already active".to_string()
                }));
            }
            match &mut entry.backend {
                Backend::Sqlite(conn) => {
                    sqlite_execute(conn, sql, &[])?;
                }
                Backend::Mysql(conn) => {
                    mysql_execute(conn, sql, None, &[])?;
                }
            }
            entry.in_transaction = !wanted_active;
            Ok(None)
        }
        "lastInsertId" => {
            let entry = registry
                .conn_mut(id)
                .expect("connection presence checked above");
            // stdlib.md: "or `null` if no such row exists (no insert has
            // been performed, or the last statement did not generate an
            // ID)" — both drivers report that case as 0.
            let raw = match &mut entry.backend {
                Backend::Sqlite(conn) => conn.last_insert_rowid(),
                Backend::Mysql(conn) => conn.last_insert_id() as i64,
            };
            Ok(Some(if raw == 0 {
                Value::Null
            } else {
                Value::Int(raw)
            }))
        }
        _ => Err(VmError::MethodNotFound(format!("{CONNECTION}.{name}"))),
    }
}

fn dispatch_statement(
    program: &Arc<Program>,
    name: &str,
    receiver: &Value,
    args: Vec<Value>,
) -> Result<Option<Value>, VmError> {
    let id = handle_of(receiver, STMT_HANDLE)?;
    match name {
        "close" => {
            if let Some(slot) = lock(program.db()).statements.get_mut(id as usize) {
                *slot = None;
            }
            return Ok(None);
        }
        "isClosed" => {
            let closed = lock(program.db()).statement_is_closed(id);
            return Ok(Some(Value::Bool(closed)));
        }
        _ => {}
    }
    let mut registry = lock(program.db());
    if registry.statement_is_closed(id) {
        return Err(sql_error(format!("{name} on a closed statement")));
    }
    // `parameterCount` reads cached state and is the one remaining method
    // stdlib.md gives no `throws` clause for.
    let param_count = registry.statements[id as usize]
        .as_ref()
        .expect("statement presence checked above")
        .param_count;
    if name == "parameterCount" {
        return Ok(Some(Value::Int(param_count as i64)));
    }
    if let Some(bound) = bind_value(name, &args)? {
        // stdlib.md § Placeholders: 0-based indices, "to align with NL array
        // and list conventions".
        let index = int_arg(&args, 0)?;
        if index < 0 || index as usize >= param_count {
            return Err(sql_error(format!(
                "bind index {index} out of range, statement has {param_count} placeholder(s)"
            )));
        }
        registry.statements[id as usize]
            .as_mut()
            .expect("statement presence checked above")
            .binds[index as usize] = Some(bound);
        return Ok(None);
    }
    match name {
        "reset" => {
            let entry = registry.statements[id as usize]
                .as_mut()
                .expect("statement presence checked above");
            entry.binds = vec![None; param_count];
            Ok(None)
        }
        "query" | "execute" => {
            let entry = registry.statements[id as usize]
                .as_ref()
                .expect("statement presence checked above");
            let conn_id = entry.conn;
            let sql = entry.sql.clone();
            let mysql_stmt = entry.mysql_stmt.clone();
            // stdlib.md: "`query`/`execute` throw `SqlException` if any
            // placeholder is left unbound".
            let mut params = Vec::with_capacity(param_count);
            for (i, bound) in entry.binds.iter().enumerate() {
                match bound {
                    Some(v) => params.push(v.clone()),
                    None => {
                        return Err(sql_error(format!(
                            "placeholder {i} of {param_count} is unbound"
                        )))
                    }
                }
            }
            let backend = &mut registry
                .conn_mut(conn_id)
                .expect("an open statement implies an open connection")
                .backend;
            if name == "execute" {
                let affected = match backend {
                    Backend::Sqlite(conn) => sqlite_execute(conn, &sql, &params)?,
                    Backend::Mysql(conn) => mysql_execute(conn, &sql, mysql_stmt, &params)?,
                };
                return Ok(Some(Value::Int(affected)));
            }
            let (columns, rows) = match backend {
                Backend::Sqlite(conn) => sqlite_query(conn, &sql, &params)?,
                Backend::Mysql(conn) => mysql_query(conn, &sql, mysql_stmt, &params)?,
            };
            let rs_id = registry.add_result_set(ResultSetEntry {
                conn: conn_id,
                columns,
                rows,
                cursor: 0,
            });
            Ok(Some(handle_object(RESULT_SET, RS_HANDLE, rs_id)))
        }
        _ => Err(VmError::MethodNotFound(format!("{STATEMENT}.{name}"))),
    }
}

/// The `bindX` family, decoded from the call's arguments — `None` when
/// `name` isn't one of them, so the caller can fall through to the rest of
/// `PreparedStatement`'s methods.
fn bind_value(name: &str, args: &[Value]) -> Result<Option<DbValue>, VmError> {
    Ok(Some(match name {
        "bindInt" => DbValue::Int(int_arg(args, 1)?),
        "bindFloat" => match args.get(1) {
            Some(Value::Float(f)) => DbValue::Float(*f),
            // An `int` literal reaching a `float` parameter is widened by
            // the compiler, but a `byte` can arrive as-is.
            Some(Value::Int(i)) => DbValue::Float(*i as f64),
            _ => return Err(VmError::Malformed("expected float argument to native call")),
        },
        "bindBool" => DbValue::Bool(bool_arg(args, 1)?),
        "bindString" => DbValue::Text(str_arg(args, 1)?),
        "bindBytes" => DbValue::Blob(bytes_arg(args, 1)?),
        "bindNull" => DbValue::Null,
        _ => return Ok(None),
    }))
}

fn dispatch_result_set(
    program: &Arc<Program>,
    name: &str,
    receiver: &Value,
    args: Vec<Value>,
) -> Result<Option<Value>, VmError> {
    let id = handle_of(receiver, RS_HANDLE)?;
    match name {
        "close" => {
            if let Some(slot) = lock(program.db()).result_sets.get_mut(id as usize) {
                *slot = None;
            }
            return Ok(None);
        }
        "isClosed" => {
            let closed = lock(program.db()).result_set_is_closed(id);
            return Ok(Some(Value::Bool(closed)));
        }
        _ => {}
    }
    let mut registry = lock(program.db());
    if registry.result_set_is_closed(id) {
        return Err(sql_error(format!("{name} on a closed result set")));
    }
    let entry = registry.result_sets[id as usize]
        .as_mut()
        .expect("result set presence checked above");
    match name {
        "next" => {
            if entry.cursor >= entry.rows.len() {
                // Exhausted. The cursor is left past the end so any `Row`
                // already handed out stays stale.
                entry.cursor = entry.rows.len() + 1;
                return Ok(Some(Value::Null));
            }
            entry.cursor += 1;
            let mut fields = HashMap::new();
            fields.insert(RS_HANDLE.to_string(), Value::Int(id));
            fields.insert(ROW_SEQ.to_string(), Value::Int(entry.cursor as i64));
            Ok(Some(Value::Object(Arc::new(Mutex::new(Object::native(
                ROW, fields,
            ))))))
        }
        "columnCount" => Ok(Some(Value::Int(entry.columns.len() as i64))),
        "columnName" => {
            let index = int_arg(&args, 0)?;
            if index < 0 || index as usize >= entry.columns.len() {
                return Err(sql_error(format!(
                    "column index {index} out of range, result set has {} column(s)",
                    entry.columns.len()
                )));
            }
            Ok(Some(Value::Str(Arc::new(
                entry.columns[index as usize].clone(),
            ))))
        }
        _ => Err(VmError::MethodNotFound(format!("{RESULT_SET}.{name}"))),
    }
}

fn dispatch_row(
    program: &Arc<Program>,
    name: &str,
    receiver: &Value,
    args: Vec<Value>,
) -> Result<Option<Value>, VmError> {
    let rs_id = handle_of(receiver, RS_HANDLE)?;
    let seq = handle_of(receiver, ROW_SEQ)?;
    let registry = lock(program.db());
    if registry.result_set_is_closed(rs_id) {
        return Err(sql_error(format!("{name} on a closed result set")));
    }
    let entry = registry.result_sets[rs_id as usize]
        .as_ref()
        .expect("result set presence checked above");
    // stdlib.md § system.db.Row: "`Row` instances are valid only until the
    // next call to `ResultSet.next()` or `ResultSet.close()`. Passing a
    // stale `Row` reference to any accessor throws `SqlException`."
    if entry.cursor as i64 != seq {
        return Err(sql_error(format!(
            "{name} on a stale row (the result set has advanced past it)"
        )));
    }
    let row = &entry.rows[seq as usize - 1];
    if name == "columnCount" {
        return Ok(Some(Value::Int(row.len() as i64)));
    }
    // stdlib.md § Column lookup by name: every typed accessor has an
    // overload taking the column name instead of the index, resolved
    // case-sensitively, first match, `SqlException` when there is no such
    // column. Which overload was written is decided here by the argument's
    // runtime shape (nl-codegen picks the same one statically, from the
    // argument's inferred type — see `nl_codegen::stdlib::is_row_accessor`).
    let index = match args.first() {
        Some(Value::Str(wanted)) => entry
            .columns
            .iter()
            .position(|c| c.as_str() == wanted.as_str())
            .ok_or_else(|| sql_error(format!("no column named '{wanted}' in this result set")))?,
        Some(Value::Int(i)) => {
            // Index form: out of range is `IndexOutOfBoundsException`
            // (runtime), per stdlib.md § system.db.Row — *not* the
            // `SqlException` the name form raises.
            if *i < 0 || *i as usize >= row.len() {
                return Err(throw_native(
                    "IndexOutOfBoundsException",
                    format!("column index {i}, row has {} column(s)", row.len()),
                ));
            }
            *i as usize
        }
        _ => {
            return Err(VmError::Malformed(
                "expected int or string argument to native call",
            ))
        }
    };
    let value = &row[index];
    match name {
        "columnType" => Ok(Some(Value::Int(value.column_type()))),
        "isNull" => Ok(Some(Value::Bool(matches!(value, DbValue::Null)))),
        // stdlib.md § system.db.Row: "Every typed accessor returns a
        // nullable union (`T|null`) — SQL `NULL` maps to NL `null`."
        _ if matches!(value, DbValue::Null) => Ok(Some(Value::Null)),
        "getInt" => Ok(Some(Value::Int(as_int(value)?))),
        "getFloat" => Ok(Some(Value::Float(as_float(value)?))),
        "getBool" => Ok(Some(Value::Bool(as_bool(value)?))),
        "getString" => Ok(Some(Value::Str(Arc::new(as_string(value)?)))),
        "getBytes" => Ok(Some(value_from_db(&DbValue::Blob(as_bytes(value)?)))),
        _ => Err(VmError::MethodNotFound(format!("{ROW}.{name}"))),
    }
}

// ---------------------------------------------------------------------------
// Typed accessors
// ---------------------------------------------------------------------------
//
// stdlib.md only pins down the two ends of the conversion scale: SQL `NULL`
// becomes NL `null` (handled by the caller), and a value that "cannot be
// represented as" the requested type throws `InvalidCastException`, with
// "text that does not parse" and "blob" given as the examples for `getInt`.
// Everything between is left to the implementation, and these accessors are
// deliberately permissive about it — SQLite is dynamically typed (an
// `INTEGER` column happily stores text, and *every* boolean is an integer,
// which stdlib.md's `ColumnType.Bool` row acknowledges), so a strict
// same-type-only rule would make `getInt`/`getBool` unusable against real
// schemas. Blobs are the one hard boundary: they never convert to a scalar,
// and only `getBytes`/`getString`-of-text produce one.

fn as_int(v: &DbValue) -> Result<i64, VmError> {
    match v {
        DbValue::Int(i) => Ok(*i),
        DbValue::Bool(b) => Ok(*b as i64),
        DbValue::Float(f) => Ok(*f as i64),
        DbValue::Text(s) => s.trim().parse::<i64>().map_err(|_| bad_cast(v, "int")),
        _ => Err(bad_cast(v, "int")),
    }
}

fn as_float(v: &DbValue) -> Result<f64, VmError> {
    match v {
        DbValue::Float(f) => Ok(*f),
        DbValue::Int(i) => Ok(*i as f64),
        DbValue::Bool(b) => Ok(*b as i64 as f64),
        DbValue::Text(s) => s.trim().parse::<f64>().map_err(|_| bad_cast(v, "float")),
        _ => Err(bad_cast(v, "float")),
    }
}

fn as_bool(v: &DbValue) -> Result<bool, VmError> {
    match v {
        DbValue::Bool(b) => Ok(*b),
        // stdlib.md § ColumnType: "Drivers without a native boolean type
        // (e.g. SQLite) report boolean columns as `Integer`" — so 0/1 must
        // read back as a bool.
        DbValue::Int(i) => Ok(*i != 0),
        DbValue::Float(f) => Ok(*f != 0.0),
        DbValue::Text(s) => match s.trim() {
            "true" | "TRUE" | "1" => Ok(true),
            "false" | "FALSE" | "0" => Ok(false),
            _ => Err(bad_cast(v, "bool")),
        },
        _ => Err(bad_cast(v, "bool")),
    }
}

fn as_string(v: &DbValue) -> Result<String, VmError> {
    match v {
        DbValue::Text(s) => Ok(s.clone()),
        DbValue::Int(i) => Ok(i.to_string()),
        DbValue::Float(f) => Ok(f.to_string()),
        DbValue::Bool(b) => Ok(b.to_string()),
        // A blob is arbitrary bytes; decoding it as text would either lose
        // data or invent replacement characters. `getBytes` is the accessor
        // for it.
        _ => Err(bad_cast(v, "string")),
    }
}

fn as_bytes(v: &DbValue) -> Result<Vec<u8>, VmError> {
    match v {
        DbValue::Blob(b) => Ok(b.clone()),
        DbValue::Text(s) => Ok(s.as_bytes().to_vec()),
        _ => Err(bad_cast(v, "byte[]")),
    }
}

// ---------------------------------------------------------------------------
// SQLite backend
// ---------------------------------------------------------------------------

impl rusqlite::types::ToSql for DbValue {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        use rusqlite::types::{ToSqlOutput, ValueRef};
        Ok(match self {
            DbValue::Null => ToSqlOutput::Borrowed(ValueRef::Null),
            DbValue::Int(i) => ToSqlOutput::Borrowed(ValueRef::Integer(*i)),
            DbValue::Float(f) => ToSqlOutput::Borrowed(ValueRef::Real(*f)),
            // SQLite has no boolean type: `true`/`false` are stored as 1/0,
            // exactly as stdlib.md § PreparedStatement.bindBool describes.
            DbValue::Bool(b) => ToSqlOutput::Borrowed(ValueRef::Integer(*b as i64)),
            DbValue::Text(s) => ToSqlOutput::Borrowed(ValueRef::Text(s.as_bytes())),
            DbValue::Blob(b) => ToSqlOutput::Borrowed(ValueRef::Blob(b)),
        })
    }
}

fn sqlite_value(v: rusqlite::types::ValueRef<'_>) -> DbValue {
    use rusqlite::types::ValueRef;
    match v {
        ValueRef::Null => DbValue::Null,
        ValueRef::Integer(i) => DbValue::Int(i),
        ValueRef::Real(f) => DbValue::Float(f),
        // A `TEXT` column that isn't valid UTF-8 is reported as a blob
        // rather than silently mangled — stdlib.md § ColumnType defines
        // `Text` as "decoded as UTF-8".
        ValueRef::Text(bytes) => match std::str::from_utf8(bytes) {
            Ok(s) => DbValue::Text(s.to_string()),
            Err(_) => DbValue::Blob(bytes.to_vec()),
        },
        ValueRef::Blob(bytes) => DbValue::Blob(bytes.to_vec()),
    }
}

fn sqlite_query(
    conn: &rusqlite::Connection,
    sql: &str,
    params: &[DbValue],
) -> Result<(Vec<String>, Vec<Vec<DbValue>>), VmError> {
    let mut stmt = conn
        .prepare_cached(sql)
        .map_err(|e| sqlite_error(&format!("prepare {sql}"), e))?;
    // Read before `query` takes `&mut stmt`: `column_names` borrows the
    // statement immutably.
    let columns: Vec<String> = stmt
        .column_names()
        .into_iter()
        .map(str::to_string)
        .collect();
    let mut rows_out = Vec::new();
    let mut rows = stmt
        .query(rusqlite::params_from_iter(params.iter()))
        .map_err(|e| sqlite_error(&format!("query {sql}"), e))?;
    while let Some(row) = rows
        .next()
        .map_err(|e| sqlite_error(&format!("query {sql}"), e))?
    {
        let mut values = Vec::with_capacity(columns.len());
        for i in 0..columns.len() {
            let raw = row
                .get_ref(i)
                .map_err(|e| sqlite_error(&format!("query {sql}"), e))?;
            values.push(sqlite_value(raw));
        }
        rows_out.push(values);
    }
    Ok((columns, rows_out))
}

fn sqlite_execute(
    conn: &rusqlite::Connection,
    sql: &str,
    params: &[DbValue],
) -> Result<i64, VmError> {
    let mut stmt = conn
        .prepare_cached(sql)
        .map_err(|e| sqlite_error(&format!("prepare {sql}"), e))?;
    // stdlib.md: "Returns the number of rows affected, or `0` for statements
    // that do not affect rows" — `DROP`/`BEGIN`/`CREATE` report 0 naturally.
    let affected = stmt
        .execute(rusqlite::params_from_iter(params.iter()))
        .map_err(|e| sqlite_error(&format!("execute {sql}"), e))?;
    Ok(affected as i64)
}

// ---------------------------------------------------------------------------
// MySQL backend
// ---------------------------------------------------------------------------

impl From<&DbValue> for mysql::Value {
    fn from(v: &DbValue) -> mysql::Value {
        match v {
            DbValue::Null => mysql::Value::NULL,
            DbValue::Int(i) => mysql::Value::Int(*i),
            DbValue::Float(f) => mysql::Value::Double(*f),
            // Same 1/0 encoding as SQLite: MySQL's `BOOL` is an alias for
            // `TINYINT(1)`.
            DbValue::Bool(b) => mysql::Value::Int(*b as i64),
            DbValue::Text(s) => mysql::Value::Bytes(s.as_bytes().to_vec()),
            DbValue::Blob(b) => mysql::Value::Bytes(b.clone()),
        }
    }
}

fn mysql_value(v: mysql::Value) -> DbValue {
    match v {
        mysql::Value::NULL => DbValue::Null,
        mysql::Value::Int(i) => DbValue::Int(i),
        mysql::Value::UInt(u) => DbValue::Int(u as i64),
        mysql::Value::Float(f) => DbValue::Float(f as f64),
        mysql::Value::Double(f) => DbValue::Float(f),
        // The wire protocol delivers every string *and* every binary column
        // as bytes; valid UTF-8 is `Text` (stdlib.md's definition), anything
        // else stays a `Blob`.
        mysql::Value::Bytes(bytes) => match String::from_utf8(bytes) {
            Ok(s) => DbValue::Text(s),
            Err(e) => DbValue::Blob(e.into_bytes()),
        },
        // `DATE`/`TIME` have no `ColumnType` of their own; rendered in their
        // usual SQL text form, which is what `getString` would produce
        // server-side anyway.
        other => DbValue::Text(other.as_sql(true).trim_matches('\'').to_string()),
    }
}

fn mysql_rows(
    result: mysql::QueryResult<'_, '_, '_, mysql::Binary>,
) -> Result<(Vec<String>, Vec<Vec<DbValue>>), mysql::Error> {
    let columns: Vec<String> = result
        .columns()
        .as_ref()
        .iter()
        .map(|c| c.name_str().into_owned())
        .collect();
    let mut rows_out = Vec::new();
    for row in result {
        let row = row?;
        rows_out.push(row.unwrap().into_iter().map(mysql_value).collect());
    }
    Ok((columns, rows_out))
}

/// `stmt` is `Some` for a `PreparedStatement` (the server-side handle
/// obtained at `prepare()` time) and `None` for `Connection.query(string)`,
/// which stdlib.md documents as parameterless constant SQL — it is still run
/// through `prep`/`exec` so both paths share one code path and the text
/// never has anything interpolated into it.
fn mysql_query(
    conn: &mut mysql::Conn,
    sql: &str,
    stmt: Option<mysql::Statement>,
    params: &[DbValue],
) -> Result<(Vec<String>, Vec<Vec<DbValue>>), VmError> {
    use mysql::prelude::Queryable;
    let stmt = match stmt {
        Some(s) => s,
        None => conn
            .prep(sql)
            .map_err(|e| mysql_error(&format!("prepare {sql}"), e))?,
    };
    let args: Vec<mysql::Value> = params.iter().map(mysql::Value::from).collect();
    let result = conn
        .exec_iter(&stmt, mysql::Params::Positional(args))
        .map_err(|e| mysql_error(&format!("query {sql}"), e))?;
    mysql_rows(result).map_err(|e| mysql_error(&format!("query {sql}"), e))
}

fn mysql_execute(
    conn: &mut mysql::Conn,
    sql: &str,
    stmt: Option<mysql::Statement>,
    params: &[DbValue],
) -> Result<i64, VmError> {
    use mysql::prelude::Queryable;
    let stmt = match stmt {
        Some(s) => s,
        None => conn
            .prep(sql)
            .map_err(|e| mysql_error(&format!("prepare {sql}"), e))?,
    };
    let args: Vec<mysql::Value> = params.iter().map(mysql::Value::from).collect();
    let result = conn
        .exec_iter(&stmt, mysql::Params::Positional(args))
        .map_err(|e| mysql_error(&format!("execute {sql}"), e))?;
    Ok(result.affected_rows() as i64)
}
