//! `system.text.json` — stdlib.md § system.text.json: a JSON ([RFC 8259](
//! https://www.rfc-editor.org/rfc/rfc8259)) document modelled as a
//! `JsonValue` tree, with `system.text.json.Json` providing the static
//! `parse`/`tryParse`/`stringify` entry points.
//!
//! ## Object representation
//!
//! The six concrete value kinds are native instance classes in the sense of
//! `native::is_native_instance_class`: real heap `Value::Object`s carrying
//! their FQCN as `class_name`, with no backing bytecode `Module` (they have
//! no `.nl` source — `nl_sema::stdlib`/`nl_codegen::stdlib` type-check and
//! emit calls against them, and `interpreter::exec_step` intercepts `NEW`,
//! `INVOKE_SPECIAL <construct>` and `INVOKE_INSTANCE` for them, exactly like
//! `system.Random` and `system.List<T>`).
//!
//! | Class | Fields |
//! |---|---|
//! | `JsonNull` | none |
//! | `JsonBool`/`JsonNumber`/`JsonString` | `value` (`Bool`/`Float`/`Str`) — public per stdlib.md |
//! | `JsonArray` | `__items__`: `Value::Array` of `JsonValue` |
//! | `JsonObject` | `__keys__`/`__values__`: two parallel `Value::Array`s |
//!
//! `JsonObject`'s parallel arrays are the same shape `native`'s
//! `system.Map<K,V>` uses, and for the same reason: insertion order is part
//! of the contract (stdlib.md: "Key order follows insertion order"), which a
//! hash map would not preserve, and JSON objects in practice are small
//! enough that O(n) key lookup is not a concern. Unlike `Map`, keys here are
//! always `string`, so lookup is plain string equality rather than
//! `equatable_equals`.
//!
//! `JsonValue` itself is abstract (never instantiated) and, per stdlib.md,
//! implements `Stringable`: `interpreter::display_string_of` routes
//! `toString()` on any of the six back into `stringify_compact` here, since
//! there is no bytecode method for `resolve_virtual_by_name` to find.
//!
//! ## Locking discipline
//!
//! Every traversal (`stringify`, `values()`, `entries()`, ...) clones the
//! backing `Vec<Value>` out of the object's `Mutex` *before* recursing into
//! its children. `Mutex` is not reentrant, so a self-referential tree
//! (`a.add(a)`, which nothing prevents — `JsonArray` is `readonly` but its
//! contents are mutable) would otherwise deadlock instead of hitting
//! `MAX_DEPTH`.
//!
//! ## Depth limit
//!
//! Both the parser and the serializer are recursive, so both are bounded by
//! `MAX_DEPTH` (there is no NL-level way to catch a Rust stack overflow, and
//! `tryParse` is documented as the safe entry point for *untrusted* input).
//! Exceeding it while parsing is an ordinary `JsonFormatException`; while
//! serializing — only reachable through a cyclic tree the user built — it is
//! an `IllegalArgumentException`, since stdlib.md declares no checked
//! exception on `stringify`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::error::VmError;
use crate::native::throw_native;
use crate::value::{lock, Object, Value};

pub const JSON: &str = "system.text.json.Json";
pub const NULL: &str = "system.text.json.JsonNull";
pub const BOOL: &str = "system.text.json.JsonBool";
pub const NUMBER: &str = "system.text.json.JsonNumber";
pub const STRING: &str = "system.text.json.JsonString";
pub const ARRAY: &str = "system.text.json.JsonArray";
pub const OBJECT: &str = "system.text.json.JsonObject";
/// The abstract base class of the six above — never a runtime `class_name`,
/// only a static type (and the `extends` parent `is_instance_of` reports).
pub const VALUE: &str = "system.text.json.JsonValue";

const ITEMS: &str = "__items__";
const KEYS: &str = "__keys__";
const VALUES: &str = "__values__";

/// Maximum nesting of a parsed or serialized document — see the module doc
/// comment. Chosen well below the point where the recursion would threaten
/// the native stack, and well above any realistic document.
const MAX_DEPTH: usize = 256;

/// Widest indent `stringify(value, indent)` honours — see its call site.
const MAX_INDENT: i64 = 64;

/// The six concrete `JsonValue` subclasses — the ones that are real runtime
/// classes (`JsonValue` itself is abstract, `Json` is a static-only utility
/// class dispatched through `native::dispatch`).
pub fn is_json_value_class(fqcn: &str) -> bool {
    matches!(fqcn, NULL | BOOL | NUMBER | STRING | ARRAY | OBJECT)
}

/// The `extends` parent of a native JSON class, for the VM's `is_instance_of`
/// (catch matching, `CHECKCAST`, the `Stringable` test in
/// `display_string_of`) — these classes have no `Module` whose `super_class`
/// it could read. Mirrors `nl_sema::class_table`/`nl_codegen::class_table`'s
/// compile-time copies of the same hierarchy.
pub fn native_parent(fqcn: &str) -> Option<&'static str> {
    if is_json_value_class(fqcn) {
        Some(VALUE)
    } else if fqcn == VALUE {
        // stdlib.md: "Implements Stringable". Modelled as the parent of the
        // root so one walk covers both the class hierarchy and the single
        // interface, like `is_instance_of`'s ordinary `extends` walk does.
        Some("Stringable")
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Construction
// ---------------------------------------------------------------------------

fn object_of(class_name: &str, fields: HashMap<String, Value>) -> Value {
    Value::Object(Arc::new(Mutex::new(Object::native(class_name, fields))))
}

fn one_field(class_name: &str, value: Value) -> Value {
    let mut fields = HashMap::new();
    fields.insert("value".to_string(), value);
    object_of(class_name, fields)
}

pub fn json_null() -> Value {
    object_of(NULL, HashMap::new())
}

pub fn json_bool(v: bool) -> Value {
    one_field(BOOL, Value::Bool(v))
}

pub fn json_number(v: f64) -> Value {
    one_field(NUMBER, Value::Float(v))
}

pub fn json_string(v: impl Into<String>) -> Value {
    one_field(STRING, Value::Str(Arc::new(v.into())))
}

pub fn json_array(items: Vec<Value>) -> Value {
    let mut fields = HashMap::new();
    fields.insert(ITEMS.to_string(), Value::Array(Arc::new(Mutex::new(items))));
    object_of(ARRAY, fields)
}

pub fn json_object(keys: Vec<Value>, values: Vec<Value>) -> Value {
    let mut fields = HashMap::new();
    fields.insert(KEYS.to_string(), Value::Array(Arc::new(Mutex::new(keys))));
    fields.insert(
        VALUES.to_string(),
        Value::Array(Arc::new(Mutex::new(values))),
    );
    object_of(OBJECT, fields)
}

/// `NEW system.text.json.JsonX` — the object is created with its fields
/// already in their empty state, so a `JsonArray`/`JsonObject` is usable
/// even though its (argument-less) `<construct>` has nothing left to do.
/// The scalar wrappers get their `value` from `construct`, below.
pub fn new_json_object(fqcn: &str) -> Value {
    match fqcn {
        ARRAY => json_array(Vec::new()),
        OBJECT => json_object(Vec::new(), Vec::new()),
        BOOL => json_bool(false),
        NUMBER => json_number(0.0),
        STRING => json_string(""),
        _ => json_null(),
    }
}

/// `INVOKE_SPECIAL system.text.json.JsonX.<construct>` — only the three
/// scalar wrappers take an argument (`JsonNull`/`JsonArray`/`JsonObject` are
/// argument-less, already fully initialized by `new_json_object`).
pub fn construct_json(receiver: &Value, fqcn: &str, args: Vec<Value>) -> Result<(), VmError> {
    let Value::Object(obj) = receiver else {
        return Err(VmError::Malformed("expected JsonValue receiver"));
    };
    let value = match (fqcn, args.first()) {
        (BOOL, Some(Value::Bool(b))) => Value::Bool(*b),
        (NUMBER, Some(Value::Float(f))) => Value::Float(*f),
        // `new JsonNumber(1)` — nl-codegen widens an `int` literal to the
        // declared `float` parameter, but an `int` can still arrive here
        // through a path that skipped the coercion (an untyped `auto`, a
        // hand-written `.nlm`); accepted rather than rejected, same
        // leniency `byte`/`int` mixing gets elsewhere in this crate.
        (NUMBER, Some(Value::Int(i))) => Value::Float(*i as f64),
        (STRING, Some(Value::Str(s))) => Value::Str(s.clone()),
        _ => return Ok(()),
    };
    lock(obj).fields.insert("value".to_string(), value);
    Ok(())
}

// ---------------------------------------------------------------------------
// Instance dispatch
// ---------------------------------------------------------------------------

fn field_of(receiver: &Value, name: &str) -> Result<Value, VmError> {
    let Value::Object(obj) = receiver else {
        return Err(VmError::Malformed("expected JsonValue receiver"));
    };
    lock(obj)
        .fields
        .get(name)
        .cloned()
        .ok_or(VmError::Malformed("malformed JsonValue object"))
}

fn backing_array(receiver: &Value, name: &str) -> Result<Arc<Mutex<Vec<Value>>>, VmError> {
    match field_of(receiver, name)? {
        Value::Array(a) => Ok(a),
        _ => Err(VmError::Malformed("malformed JsonValue object")),
    }
}

fn class_of(receiver: &Value) -> Result<String, VmError> {
    let Value::Object(obj) = receiver else {
        return Err(VmError::Malformed("expected JsonValue receiver"));
    };
    let name = lock(obj).class_name.clone();
    Ok(name)
}

/// stdlib.md § JsonValue: `asBool`/`asNumber`/`asString`/`asArray`/`asObject`
/// "Throws `InvalidCastException` if this is not `JsonX`" — the message
/// names both the actual and the requested kind, since the call site itself
/// (`root.asObject()`) carries no other clue about what was in the document.
fn wrong_kind(actual: &str, wanted: &str) -> VmError {
    throw_native(
        "InvalidCastException",
        format!("{} is not a {}", short_name(actual), short_name(wanted)),
    )
}

fn short_name(fqcn: &str) -> &str {
    fqcn.rsplit('.').next().unwrap_or(fqcn)
}

fn str_arg(args: &[Value], index: usize) -> Result<Arc<String>, VmError> {
    match args.get(index) {
        Some(Value::Str(s)) => Ok(s.clone()),
        Some(Value::Null) => Err(throw_native(
            "NullPointerException",
            "null string argument to a system.text.json call",
        )),
        _ => Err(VmError::Malformed("expected string argument")),
    }
}

fn int_arg(args: &[Value], index: usize) -> Result<i64, VmError> {
    match args.get(index) {
        Some(Value::Int(i)) => Ok(*i),
        _ => Err(VmError::Malformed("expected int argument")),
    }
}

/// A `JsonValue` argument (`array.add(v)`, `object.set(k, v)`): non-null per
/// stdlib.md's signatures — a JSON `null` is a `JsonNull` *instance*, never
/// NL `null` (see the "Absent key vs. JSON null" note in the spec).
fn value_arg(args: &[Value], index: usize) -> Result<Value, VmError> {
    match args.get(index) {
        Some(Value::Null) | None => Err(throw_native(
            "NullPointerException",
            "null JsonValue argument (use new system.text.json.JsonNull() for JSON null)",
        )),
        Some(v) => Ok(v.clone()),
    }
}

fn key_index(keys: &Arc<Mutex<Vec<Value>>>, key: &str) -> Option<usize> {
    lock(keys).iter().position(|k| match k {
        Value::Str(s) => s.as_str() == key,
        _ => false,
    })
}

/// `INVOKE_INSTANCE` against any of the six concrete `JsonValue` classes —
/// the `JsonValue` base methods (`isX`/`asX`/`toString`) are handled for
/// every receiver class first, then the `JsonArray`/`JsonObject` specific
/// ones.
pub fn dispatch_json_instance(
    name: &str,
    receiver: &Value,
    args: Vec<Value>,
) -> Result<Option<Value>, VmError> {
    let class = class_of(receiver)?;

    match name {
        "toString" => return Ok(Some(Value::Str(Arc::new(stringify(receiver, None)?)))),
        "isNull" => return Ok(Some(Value::Bool(class == NULL))),
        "isBool" => return Ok(Some(Value::Bool(class == BOOL))),
        "isNumber" => return Ok(Some(Value::Bool(class == NUMBER))),
        "isString" => return Ok(Some(Value::Bool(class == STRING))),
        "isArray" => return Ok(Some(Value::Bool(class == ARRAY))),
        "isObject" => return Ok(Some(Value::Bool(class == OBJECT))),
        "asBool" | "asNumber" | "asString" => {
            let wanted = match name {
                "asBool" => BOOL,
                "asNumber" => NUMBER,
                _ => STRING,
            };
            if class != wanted {
                return Err(wrong_kind(&class, wanted));
            }
            return Ok(Some(field_of(receiver, "value")?));
        }
        "asArray" | "asObject" => {
            let wanted = if name == "asArray" { ARRAY } else { OBJECT };
            if class != wanted {
                return Err(wrong_kind(&class, wanted));
            }
            return Ok(Some(receiver.clone()));
        }
        _ => {}
    }

    match class.as_str() {
        ARRAY => dispatch_array(name, receiver, args),
        OBJECT => dispatch_object(name, receiver, args),
        _ => Err(VmError::MethodNotFound(format!("{class}.{name}"))),
    }
}

fn dispatch_array(
    name: &str,
    receiver: &Value,
    args: Vec<Value>,
) -> Result<Option<Value>, VmError> {
    let items = backing_array(receiver, ITEMS)?;
    match name {
        "length" => Ok(Some(Value::Int(lock(&items).len() as i64))),
        "get" | "set" => {
            let index = int_arg(&args, 0)?;
            let len = lock(&items).len() as i64;
            if index < 0 || index >= len {
                return Err(throw_native(
                    "IndexOutOfBoundsException",
                    format!("index {index}, JsonArray length {len}"),
                ));
            }
            if name == "get" {
                let item = lock(&items)[index as usize].clone();
                Ok(Some(item))
            } else {
                let value = value_arg(&args, 1)?;
                lock(&items)[index as usize] = value;
                Ok(None)
            }
        }
        "add" => {
            let value = value_arg(&args, 0)?;
            lock(&items).push(value);
            Ok(None)
        }
        // stdlib.md: "Returns a *snapshot* array of all elements" — a fresh
        // `Arc`, so mutating the result can't desync the JsonArray (same
        // rule as `system.Map.keys()`/`values()`).
        "values" => Ok(Some(Value::Array(Arc::new(Mutex::new(
            lock(&items).clone(),
        ))))),
        _ => Err(VmError::MethodNotFound(format!("{ARRAY}.{name}"))),
    }
}

fn dispatch_object(
    name: &str,
    receiver: &Value,
    args: Vec<Value>,
) -> Result<Option<Value>, VmError> {
    let keys = backing_array(receiver, KEYS)?;
    let values = backing_array(receiver, VALUES)?;
    match name {
        "size" => Ok(Some(Value::Int(lock(&keys).len() as i64))),
        // stdlib.md § Absent key vs. JSON `null`: NL `null` *only* for an
        // absent key — a key holding JSON `null` yields a `JsonNull`.
        "get" => {
            let key = str_arg(&args, 0)?;
            Ok(Some(match key_index(&keys, &key) {
                Some(i) => lock(&values)[i].clone(),
                None => Value::Null,
            }))
        }
        "has" => {
            let key = str_arg(&args, 0)?;
            Ok(Some(Value::Bool(key_index(&keys, &key).is_some())))
        }
        "set" => {
            let key = str_arg(&args, 0)?;
            let value = value_arg(&args, 1)?;
            match key_index(&keys, &key) {
                // Update in place: an existing key keeps its original
                // position, per stdlib.md's insertion-order guarantee.
                Some(i) => lock(&values)[i] = value,
                None => {
                    lock(&keys).push(Value::Str(key));
                    lock(&values).push(value);
                }
            }
            Ok(None)
        }
        "remove" => {
            let key = str_arg(&args, 0)?;
            Ok(Some(Value::Bool(match key_index(&keys, &key) {
                Some(i) => {
                    lock(&keys).remove(i);
                    lock(&values).remove(i);
                    true
                }
                None => false,
            })))
        }
        "keys" => Ok(Some(Value::Array(Arc::new(Mutex::new(
            lock(&keys).clone(),
        ))))),
        // stdlib.md § system.MapEntry — the same two-public-field result
        // objects `system.Map.entries()` produces, under the matching
        // mangled instantiation name.
        "entries" => {
            let entry_class = format!("system.MapEntry<string, {VALUE}>");
            let entries: Vec<Value> = lock(&keys)
                .iter()
                .zip(lock(&values).iter())
                .map(|(k, v)| {
                    let mut fields = HashMap::new();
                    fields.insert("key".to_string(), k.clone());
                    fields.insert("value".to_string(), v.clone());
                    object_of(&entry_class, fields)
                })
                .collect();
            Ok(Some(Value::Array(Arc::new(Mutex::new(entries)))))
        }
        _ => Err(VmError::MethodNotFound(format!("{OBJECT}.{name}"))),
    }
}

/// `INVOKE_STATIC system.text.json.Json.<name>` — see `native::dispatch`,
/// which routes here.
pub fn dispatch_static(name: &str, args: Vec<Value>) -> Result<Option<Value>, VmError> {
    match (name, args.len()) {
        ("parse", 1) => {
            let text = str_arg(&args, 0)?;
            Ok(Some(parse(&text).map_err(throw_format)?))
        }
        // stdlib.md: "returns `null` instead of throwing when `text` is not
        // valid JSON" — only a *format* error is swallowed this way.
        ("tryParse", 1) => {
            let text = str_arg(&args, 0)?;
            Ok(Some(parse(&text).unwrap_or(Value::Null)))
        }
        ("stringify", 1) => {
            let value = value_arg(&args, 0)?;
            Ok(Some(Value::Str(Arc::new(stringify(&value, None)?))))
        }
        ("stringify", 2) => {
            let value = value_arg(&args, 0)?;
            let indent = int_arg(&args, 1)?;
            // A non-positive indent has no pretty form to produce; treated
            // as the compact overload rather than as an error (stdlib.md
            // doesn't define it). The upper bound is a memory guard, not a
            // feature: `indent * depth` spaces are written on every line,
            // and nothing else stops `stringify(v, 1000000000)` from asking
            // for a terabyte of padding.
            let indent = (indent > 0).then(|| indent.min(MAX_INDENT) as usize);
            Ok(Some(Value::Str(Arc::new(stringify(&value, indent)?))))
        }
        _ => Err(VmError::MethodNotFound(format!("{JSON}.{name}"))),
    }
}

// ---------------------------------------------------------------------------
// Serialization
// ---------------------------------------------------------------------------

/// `indent = None` is stdlib.md's compact `stringify(value)`; `Some(n)` the
/// pretty `stringify(value, n)`.
pub fn stringify(value: &Value, indent: Option<usize>) -> Result<String, VmError> {
    let mut out = String::new();
    write_value(&mut out, value, indent, 0)?;
    Ok(out)
}

/// `Stringable::toString()` on a `JsonValue` — stdlib.md: "Compact JSON
/// serialization of this value". Called by `interpreter::display_string_of`
/// (string concatenation, `(string)` casts, `print`/`println`), which has no
/// bytecode method to dispatch to for these classes.
pub fn stringify_compact(value: &Value) -> Result<String, VmError> {
    stringify(value, None)
}

fn write_value(
    out: &mut String,
    value: &Value,
    indent: Option<usize>,
    depth: usize,
) -> Result<(), VmError> {
    if depth > MAX_DEPTH {
        return Err(throw_native(
            "IllegalArgumentException",
            format!("JSON value nests deeper than {MAX_DEPTH} levels (cyclic?)"),
        ));
    }
    let class = class_of(value)?;
    match class.as_str() {
        NULL => out.push_str("null"),
        BOOL => match field_of(value, "value")? {
            Value::Bool(b) => out.push_str(if b { "true" } else { "false" }),
            _ => return Err(VmError::Malformed("malformed JsonBool")),
        },
        NUMBER => match field_of(value, "value")? {
            Value::Float(f) => out.push_str(&format_number(f)),
            Value::Int(i) => out.push_str(&format_number(i as f64)),
            _ => return Err(VmError::Malformed("malformed JsonNumber")),
        },
        STRING => match field_of(value, "value")? {
            Value::Str(s) => write_escaped(out, &s),
            _ => return Err(VmError::Malformed("malformed JsonString")),
        },
        ARRAY => {
            // Cloned out of the lock before recursing — see the module doc
            // comment's locking-discipline note.
            let items = lock(&*backing_array(value, ITEMS)?).clone();
            if items.is_empty() {
                out.push_str("[]");
                return Ok(());
            }
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_newline_indent(out, indent, depth + 1);
                write_value(out, item, indent, depth + 1)?;
            }
            write_newline_indent(out, indent, depth);
            out.push(']');
        }
        OBJECT => {
            let keys = lock(&*backing_array(value, KEYS)?).clone();
            let values = lock(&*backing_array(value, VALUES)?).clone();
            if keys.is_empty() {
                out.push_str("{}");
                return Ok(());
            }
            out.push('{');
            for (i, (k, v)) in keys.iter().zip(values.iter()).enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_newline_indent(out, indent, depth + 1);
                match k {
                    Value::Str(s) => write_escaped(out, s),
                    _ => return Err(VmError::Malformed("malformed JsonObject key")),
                }
                out.push(':');
                if indent.is_some() {
                    out.push(' ');
                }
                write_value(out, v, indent, depth + 1)?;
            }
            write_newline_indent(out, indent, depth);
            out.push('}');
        }
        _ => return Err(VmError::Malformed("expected a JsonValue")),
    }
    Ok(())
}

fn write_newline_indent(out: &mut String, indent: Option<usize>, depth: usize) {
    if let Some(width) = indent {
        out.push('\n');
        for _ in 0..width * depth {
            out.push(' ');
        }
    }
}

/// JSON has a single numeric type, so a whole `float` is written without a
/// fractional part (`1`, not `1.0`) — the round-trip JavaScript's
/// `JSON.stringify` produces, and what makes `parse(stringify(x))` stable.
/// Rust's `{}` for `f64` already emits the shortest representation that
/// round-trips for everything else.
///
/// `NaN`/infinity are not JSON values (RFC 8259 § 6). They can't come out of
/// `parse`, only out of a user-built `new JsonNumber(...)`; written as `null`,
/// matching `JSON.stringify`, rather than emitting a document no parser
/// would accept.
fn format_number(f: f64) -> String {
    if !f.is_finite() {
        return "null".to_string();
    }
    if f == f.trunc() && f.abs() < 1e17 {
        return format!("{}", f as i64);
    }
    format!("{f}")
}

/// RFC 8259 § 7: `"` and `\` must be escaped, as must every control
/// character below `0x20` — the five with a short form get it, the rest go
/// out as `\u00XX`. Everything else (including non-ASCII) is emitted as
/// literal UTF-8, which the spec allows and which keeps the output readable.
fn write_escaped(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// A `JsonFormatException`'s payload — stdlib.md § Exception table: "carries
/// `line`, `column`, `expectedToken`, `foundToken`".
struct JsonError {
    line: i64,
    column: i64,
    expected: String,
    found: String,
}

/// Builds the `JsonFormatException` object directly, like every other
/// VM-raised exception (`native::throw_native`), with the four extra fields
/// the prelude class declares on top of `Exception.message`.
fn throw_format(e: JsonError) -> VmError {
    let mut fields = crate::native::exception_fields(format!(
        "line {}, column {}: expected {} but found {}",
        e.line, e.column, e.expected, e.found
    ));
    fields.insert("line".to_string(), Value::Int(e.line));
    fields.insert("column".to_string(), Value::Int(e.column));
    fields.insert(
        "expectedToken".to_string(),
        Value::Str(Arc::new(e.expected)),
    );
    fields.insert("foundToken".to_string(), Value::Str(Arc::new(e.found)));
    VmError::Thrown(object_of("JsonFormatException", fields))
}

struct Parser {
    /// `char`s rather than bytes so `column` counts characters (a non-ASCII
    /// string literal earlier on the line would otherwise skew every column
    /// reported after it).
    chars: Vec<char>,
    pos: usize,
    line: i64,
    column: i64,
}

/// RFC 8259 recursive-descent parse of a whole document — trailing content
/// after the top-level value is an error, as the grammar requires.
fn parse(text: &str) -> Result<Value, JsonError> {
    let mut p = Parser {
        chars: text.chars().collect(),
        pos: 0,
        line: 1,
        column: 1,
    };
    let value = p.parse_value(0)?;
    p.skip_ws();
    if p.peek().is_some() {
        return Err(p.error("end of input"));
    }
    Ok(value)
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += 1;
        if c == '\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
        Some(c)
    }

    /// RFC 8259 § 2 whitespace: space, horizontal tab, line feed, carriage
    /// return — nothing else (a Unicode space would be an error, as it is
    /// for every conforming parser).
    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(' ' | '\t' | '\n' | '\r')) {
            self.bump();
        }
    }

    /// The error for "something other than `expected` sits at the cursor".
    /// `foundToken` is the offending character itself, quoted, or the
    /// literal `end of input`.
    fn error(&self, expected: &str) -> JsonError {
        JsonError {
            line: self.line,
            column: self.column,
            expected: expected.to_string(),
            found: match self.peek() {
                Some(c) => format!("'{c}'"),
                None => "end of input".to_string(),
            },
        }
    }

    fn expect(&mut self, c: char, expected: &str) -> Result<(), JsonError> {
        if self.peek() == Some(c) {
            self.bump();
            Ok(())
        } else {
            Err(self.error(expected))
        }
    }

    fn parse_value(&mut self, depth: usize) -> Result<Value, JsonError> {
        if depth > MAX_DEPTH {
            return Err(JsonError {
                line: self.line,
                column: self.column,
                expected: format!("at most {MAX_DEPTH} levels of nesting"),
                found: "a deeper nesting".to_string(),
            });
        }
        self.skip_ws();
        match self.peek() {
            Some('{') => self.parse_object(depth),
            Some('[') => self.parse_array(depth),
            Some('"') => Ok(json_string(self.parse_string()?)),
            Some('t') => self.keyword("true", json_bool(true)),
            Some('f') => self.keyword("false", json_bool(false)),
            Some('n') => self.keyword("null", json_null()),
            Some(c) if c == '-' || c.is_ascii_digit() => self.parse_number(),
            _ => Err(self.error("value")),
        }
    }

    /// One of the three literal keywords. The error points at the character
    /// that diverged rather than at the start of the token — `[tru]` reports
    /// `expected true but found ']'` at the `]`, which is where the document
    /// actually has to change.
    fn keyword(&mut self, word: &str, value: Value) -> Result<Value, JsonError> {
        for expected in word.chars() {
            if self.peek() != Some(expected) {
                return Err(self.error(word));
            }
            self.bump();
        }
        Ok(value)
    }

    fn parse_array(&mut self, depth: usize) -> Result<Value, JsonError> {
        self.expect('[', "'['")?;
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(']') {
            self.bump();
            return Ok(json_array(items));
        }
        loop {
            items.push(self.parse_value(depth + 1)?);
            self.skip_ws();
            match self.peek() {
                Some(',') => {
                    self.bump();
                }
                Some(']') => {
                    self.bump();
                    return Ok(json_array(items));
                }
                _ => return Err(self.error("',' or ']'")),
            }
        }
    }

    fn parse_object(&mut self, depth: usize) -> Result<Value, JsonError> {
        self.expect('{', "'{'")?;
        let mut keys: Vec<Value> = Vec::new();
        let mut values: Vec<Value> = Vec::new();
        self.skip_ws();
        if self.peek() == Some('}') {
            self.bump();
            return Ok(json_object(keys, values));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some('"') {
                return Err(self.error("'\"'"));
            }
            let key = self.parse_string()?;
            self.skip_ws();
            self.expect(':', "':'")?;
            let value = self.parse_value(depth + 1)?;
            // RFC 8259 § 4 leaves duplicate names undefined; last one wins
            // (what every mainstream parser does), and the first
            // occurrence's position is kept so key order stays stable.
            match keys.iter().position(|k| match k {
                Value::Str(s) => s.as_str() == key,
                _ => false,
            }) {
                Some(i) => values[i] = value,
                None => {
                    keys.push(Value::Str(Arc::new(key)));
                    values.push(value);
                }
            }
            self.skip_ws();
            match self.peek() {
                Some(',') => {
                    self.bump();
                }
                Some('}') => {
                    self.bump();
                    return Ok(json_object(keys, values));
                }
                _ => return Err(self.error("',' or '}'")),
            }
        }
    }

    fn parse_string(&mut self) -> Result<String, JsonError> {
        self.expect('"', "'\"'")?;
        let mut out = String::new();
        loop {
            match self.bump() {
                None => return Err(self.error("'\"'")),
                Some('"') => return Ok(out),
                Some('\\') => out.push(self.parse_escape()?),
                // RFC 8259 § 7: unescaped control characters are not
                // allowed inside a string.
                Some(c) if (c as u32) < 0x20 => {
                    return Err(JsonError {
                        line: self.line,
                        column: self.column,
                        expected: "an escape sequence".to_string(),
                        found: format!("control character U+{:04X}", c as u32),
                    })
                }
                Some(c) => out.push(c),
            }
        }
    }

    fn parse_escape(&mut self) -> Result<char, JsonError> {
        match self.bump() {
            Some('"') => Ok('"'),
            Some('\\') => Ok('\\'),
            Some('/') => Ok('/'),
            Some('b') => Ok('\u{8}'),
            Some('f') => Ok('\u{c}'),
            Some('n') => Ok('\n'),
            Some('r') => Ok('\r'),
            Some('t') => Ok('\t'),
            Some('u') => self.parse_unicode_escape(),
            _ => {
                self.pos = self.pos.saturating_sub(1);
                Err(self.error("an escape sequence"))
            }
        }
    }

    /// `\uXXXX`, including the surrogate pair form for characters outside
    /// the BMP (`😀`) that RFC 8259 § 7 prescribes. An unpaired
    /// surrogate is not a character; it is replaced with U+FFFD rather than
    /// rejected, so one malformed escape doesn't fail an otherwise usable
    /// document (the same lossy stance `lossy_line` takes in `native`).
    fn parse_unicode_escape(&mut self) -> Result<char, JsonError> {
        let high = self.parse_hex4()?;
        if !(0xD800..0xDC00).contains(&high) {
            return Ok(char::from_u32(high).unwrap_or('\u{FFFD}'));
        }
        let save = (self.pos, self.line, self.column);
        if self.peek() == Some('\\') {
            self.bump();
            if self.peek() == Some('u') {
                self.bump();
                let low = self.parse_hex4()?;
                if (0xDC00..0xE000).contains(&low) {
                    let combined = 0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00);
                    return Ok(char::from_u32(combined).unwrap_or('\u{FFFD}'));
                }
            }
        }
        (self.pos, self.line, self.column) = save;
        Ok('\u{FFFD}')
    }

    fn parse_hex4(&mut self) -> Result<u32, JsonError> {
        let mut value = 0u32;
        for _ in 0..4 {
            let Some(digit) = self.peek().and_then(|c| c.to_digit(16)) else {
                return Err(self.error("four hexadecimal digits"));
            };
            self.bump();
            value = value * 16 + digit;
        }
        Ok(value)
    }

    /// RFC 8259 § 6: `-? int frac? exp?`, with no leading `+`, no leading
    /// zeroes and at least one digit after `.`/`e` — all stricter than
    /// Rust's own `f64::from_str`, hence the explicit scan.
    fn parse_number(&mut self) -> Result<Value, JsonError> {
        let start = self.pos;
        if self.peek() == Some('-') {
            self.bump();
        }
        match self.peek() {
            Some('0') => {
                self.bump();
            }
            Some(c) if c.is_ascii_digit() => {
                while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                    self.bump();
                }
            }
            _ => return Err(self.error("a digit")),
        }
        if self.peek() == Some('.') {
            self.bump();
            if !matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                return Err(self.error("a digit"));
            }
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                self.bump();
            }
        }
        if matches!(self.peek(), Some('e' | 'E')) {
            self.bump();
            if matches!(self.peek(), Some('+' | '-')) {
                self.bump();
            }
            if !matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                return Err(self.error("a digit"));
            }
            while matches!(self.peek(), Some(c) if c.is_ascii_digit()) {
                self.bump();
            }
        }
        let text: String = self.chars[start..self.pos].iter().collect();
        // Unparseable is unreachable for the grammar above; an out-of-range
        // magnitude yields ±inf, which stdlib.md's "same trade-off as
        // JavaScript's JSON.parse" covers.
        match text.parse::<f64>() {
            Ok(f) => Ok(json_number(f)),
            Err(_) => Err(self.error("a number")),
        }
    }
}
