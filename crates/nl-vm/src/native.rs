//! Native bindings for the `system.*` stdlib classes — vm.md § Standard
//! library binding: "calling `system.Out.print(s)` is an `INVOKE_STATIC`
//! like any other — the VM intercepts the call and runs the native code."
//! `interpreter::exec_step`'s `INVOKE_STATIC` arm calls `dispatch` for any
//! class name `is_native_class` accepts, before ever consulting `Program`'s
//! module map (these classes have no backing bytecode `Module` — see
//! `nl_codegen::stdlib`/`nl_sema::stdlib`, which are what type-check and
//! emit calls against them).
//!
//! Only part of stdlib.md is covered so far (PLAN.md Phase 6): output
//! (`system.Out`/`system.Err`), `system.In.readLine`, int/float/bool
//! parsing/formatting, `system.String` (instance methods on `string`
//! values and their static equivalents — both compile to the same
//! `INVOKE_STATIC system.String.<name>`, see `nl_codegen::stdlib`), and
//! `system.List<T>`/`system.Map<K,V>` (see the section below). File I/O,
//! threads, etc. are future work.
//!
//! ## `system.List<T>` / `system.Map<K,V>`
//!
//! Unlike every native class above, these are real heap objects created
//! with `new` (vm.md § Templates (monomorphization) — "native template
//! classes"), not static-only utility classes. `interpreter::exec_step`
//! intercepts all three opcodes that touch them — `NEW` (`new_generic_object`
//! instead of the usual module-based field walk), `INVOKE_SPECIAL` on
//! `<construct>` (`construct_generic`), and `INVOKE_INSTANCE` keyed by the
//! *receiver's* runtime class (`dispatch_instance`) — via
//! `is_native_generic_class`, mirroring how `is_native_class` intercepts
//! `INVOKE_STATIC` for the utility classes. `nl_sema`/`nl_codegen`'s
//! `native_generics` modules recover each instantiation's concrete element
//! type(s) by parsing the mangled FQCN (e.g. `"system.Map<string, int>"`);
//! this module only needs the *values*, not their static types, so it
//! doesn't need that parsing.
//!
//! Representation: a `List<T>` instance is a plain `Value::Object` with one
//! field, `"__data__"`, holding the backing `Value::Array`. A `Map<K,V>`
//! instance has two parallel array fields, `"__keys__"`/`"__values__"` (same
//! index in both = one entry) — chosen over a real hash map because key
//! equality follows `values_equal` (§ below), which isn't `Hash`-compatible
//! in general (e.g. float, or reference-identity for plain objects), and
//! map sizes in test programs are small enough that O(n) lookup is not a
//! concern. `keys()`/`values()` return a *copy* of the backing array (a
//! fresh `Rc`), not a live view — mutating the returned array must not
//! desync it from the map, per stdlib.md's "Returns an array containing".
//!
//! Key/element equality for `contains`/map lookups reuses
//! `interpreter::values_equal` (primitives and `string` by value,
//! everything else by reference identity) — this is the same rule
//! stdlib.md documents as the *fallback* for types that don't implement
//! `ValueEquatable`; `ValueEquatable` itself is not implemented, so that
//! optimization never kicks in (reference types with structural key/element
//! equality always fall back to identity here).
//!
//! Not implemented (PLAN.md Phase 6 gap): `system.List`'s `T[] initial`
//! constructor works, but `entries()`/`forEach` on `Map` do not (they need
//! a synthetic `MapEntry<K,V>` class and closures-as-native-callbacks,
//! neither of which exist yet), and neither collection supports the
//! for-each loop (`for (const auto x : list)`) — vm.md's desugaring for
//! that relies on `entries()` for maps and hasn't been wired into
//! nl-codegen for either collection.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::error::VmError;
use crate::interpreter::values_equal;
use crate::program::Program;
use crate::value::{Object, Value};

pub fn is_native_class(fqcn: &str) -> bool {
    matches!(
        fqcn,
        "system.Out" | "system.Err" | "system.In" | "system.Int" | "system.Float" | "system.Bool" | "system.String"
    )
}

/// Dispatches one native call. `args` has already been popped off the
/// operand stack by the caller, in declaration order. Returns `Ok(None)`
/// for a `void` native (nothing to push back).
pub fn dispatch(program: &Program, fqcn: &str, name: &str, mut args: Vec<Value>) -> Result<Option<Value>, VmError> {
    match (fqcn, name) {
        ("system.Out", "print") => {
            program.write_stdout(&expect_str(&mut args)?);
            Ok(None)
        }
        ("system.Out", "println") => {
            let mut s = expect_str(&mut args)?;
            s.push('\n');
            program.write_stdout(&s);
            Ok(None)
        }
        ("system.Err", "print") => {
            program.write_stderr(&expect_str(&mut args)?);
            Ok(None)
        }
        ("system.Err", "println") => {
            let mut s = expect_str(&mut args)?;
            s.push('\n');
            program.write_stderr(&s);
            Ok(None)
        }
        ("system.In", "readLine") => {
            let mut line = String::new();
            match std::io::stdin().read_line(&mut line) {
                Ok(0) => Ok(Some(Value::Null)), // EOF
                Ok(_) => {
                    if line.ends_with('\n') {
                        line.pop();
                        if line.ends_with('\r') {
                            line.pop();
                        }
                    }
                    Ok(Some(Value::Str(Rc::new(line))))
                }
                Err(e) => Err(VmError::Io(e)),
            }
        }
        ("system.Int", "parse") => match expect_str(&mut args)?.trim().parse::<i64>() {
            Ok(v) => Ok(Some(Value::Int(v))),
            Err(_) => Err(throw_format_error("invalid int literal")),
        },
        ("system.Int", "tryParse") => match expect_str(&mut args)?.trim().parse::<i64>() {
            Ok(v) => Ok(Some(Value::Int(v))),
            Err(_) => Ok(Some(Value::Null)),
        },
        ("system.Int", "toString") => Ok(Some(Value::Str(Rc::new(expect_int(&mut args)?.to_string())))),
        ("system.Float", "parse") => match expect_str(&mut args)?.trim().parse::<f64>() {
            Ok(v) => Ok(Some(Value::Float(v))),
            Err(_) => Err(throw_format_error("invalid float literal")),
        },
        ("system.Float", "tryParse") => match expect_str(&mut args)?.trim().parse::<f64>() {
            Ok(v) => Ok(Some(Value::Float(v))),
            Err(_) => Ok(Some(Value::Null)),
        },
        ("system.Float", "toString") => Ok(Some(Value::Str(Rc::new(expect_float(&mut args)?.to_string())))),
        ("system.Bool", "parse") => match expect_str(&mut args)?.as_str() {
            "true" => Ok(Some(Value::Bool(true))),
            "false" => Ok(Some(Value::Bool(false))),
            _ => Err(throw_native("IllegalArgumentException", "expected \"true\" or \"false\"")),
        },
        ("system.Bool", "tryParse") => match expect_str(&mut args)?.as_str() {
            "true" => Ok(Some(Value::Bool(true))),
            "false" => Ok(Some(Value::Bool(false))),
            _ => Ok(Some(Value::Null)),
        },
        ("system.Bool", "toString") => Ok(Some(Value::Str(Rc::new(expect_bool(&mut args)?.to_string())))),
        // stdlib.md § system.String — `args[0]` is always the receiver
        // (whether the call came from `text.trim()` or the equivalent
        // static `system.String.trim(text)`, see nl_codegen::stdlib's doc
        // comment); indexed rather than popped since several of these take
        // more than one argument and popping would read them back to
        // front. Character positions are counted in `char`s, not bytes
        // (specs.md: "A character is represented as a string of length
        // 1").
        ("system.String", "length") => Ok(Some(Value::Int(str_at(&args, 0)?.chars().count() as i64))),
        ("system.String", "charAt") => {
            let chars: Vec<char> = str_at(&args, 0)?.chars().collect();
            let idx = int_at(&args, 1)?;
            if idx < 0 || idx as usize >= chars.len() {
                return Err(throw_native("IndexOutOfBoundsException", format!("index {idx}, length {}", chars.len())));
            }
            Ok(Some(Value::Str(Rc::new(chars[idx as usize].to_string()))))
        }
        ("system.String", "substring") => {
            let chars: Vec<char> = str_at(&args, 0)?.chars().collect();
            let start = int_at(&args, 1)?;
            let end = if args.len() >= 3 { int_at(&args, 2)? } else { chars.len() as i64 };
            if start < 0 || end < start || end as usize > chars.len() {
                return Err(throw_native(
                    "IndexOutOfBoundsException",
                    format!("start {start}, end {end}, length {}", chars.len()),
                ));
            }
            let sub: String = chars[start as usize..end as usize].iter().collect();
            Ok(Some(Value::Str(Rc::new(sub))))
        }
        ("system.String", "indexOf") => {
            let haystack = str_at(&args, 0)?;
            let needle = str_at(&args, 1)?;
            let from = if args.len() >= 3 { int_at(&args, 2)?.max(0) as usize } else { 0 };
            Ok(Some(Value::Int(char_index_of(&haystack, &needle, from).unwrap_or(-1))))
        }
        ("system.String", "contains") => Ok(Some(Value::Bool(str_at(&args, 0)?.contains(&str_at(&args, 1)?)))),
        ("system.String", "toUpperCase") => Ok(Some(Value::Str(Rc::new(str_at(&args, 0)?.to_uppercase())))),
        ("system.String", "toLowerCase") => Ok(Some(Value::Str(Rc::new(str_at(&args, 0)?.to_lowercase())))),
        ("system.String", "replace") => {
            let s = str_at(&args, 0)?;
            let from = str_at(&args, 1)?;
            let to = str_at(&args, 2)?;
            Ok(Some(Value::Str(Rc::new(s.replace(&from, &to)))))
        }
        ("system.String", "startsWith") => Ok(Some(Value::Bool(str_at(&args, 0)?.starts_with(&str_at(&args, 1)?)))),
        ("system.String", "endsWith") => Ok(Some(Value::Bool(str_at(&args, 0)?.ends_with(&str_at(&args, 1)?)))),
        ("system.String", "trim") => Ok(Some(Value::Str(Rc::new(str_at(&args, 0)?.trim().to_string())))),
        ("system.String", "split") => {
            let s = str_at(&args, 0)?;
            let delim = str_at(&args, 1)?;
            let parts: Vec<Value> = s.split(delim.as_str()).map(|p| Value::Str(Rc::new(p.to_string()))).collect();
            Ok(Some(Value::Array(Rc::new(RefCell::new(parts)))))
        }
        _ => Err(VmError::MethodNotFound(format!("{fqcn}.{name}"))),
    }
}

fn str_at(args: &[Value], i: usize) -> Result<String, VmError> {
    match args.get(i) {
        Some(Value::Str(s)) => Ok((**s).clone()),
        _ => Err(VmError::Malformed("expected string argument to native call")),
    }
}

fn int_at(args: &[Value], i: usize) -> Result<i64, VmError> {
    args.get(i).and_then(|v| v.as_int()).ok_or(VmError::Malformed("expected int argument to native call"))
}

/// Char-index (not byte-index) of the first occurrence of `needle` in
/// `haystack` at or after char position `from`, or `None`. An empty
/// `needle` matches at `from` itself, mirroring `str::find`'s behavior.
fn char_index_of(haystack: &str, needle: &str, from: usize) -> Option<i64> {
    let hay: Vec<char> = haystack.chars().collect();
    let needle: Vec<char> = needle.chars().collect();
    if needle.is_empty() {
        return if from <= hay.len() { Some(from as i64) } else { None };
    }
    if from > hay.len() || needle.len() > hay.len() {
        return None;
    }
    (from..=hay.len() - needle.len()).find(|&start| hay[start..start + needle.len()] == needle[..]).map(|s| s as i64)
}

fn expect_str(args: &mut Vec<Value>) -> Result<String, VmError> {
    match args.pop() {
        Some(Value::Str(s)) => Ok((*s).clone()),
        _ => Err(VmError::Malformed("expected string argument to native call")),
    }
}

fn expect_int(args: &mut Vec<Value>) -> Result<i64, VmError> {
    args.pop().and_then(|v| v.as_int()).ok_or(VmError::Malformed("expected int argument to native call"))
}

fn expect_float(args: &mut Vec<Value>) -> Result<f64, VmError> {
    args.pop().and_then(|v| v.as_float()).ok_or(VmError::Malformed("expected float argument to native call"))
}

fn expect_bool(args: &mut Vec<Value>) -> Result<bool, VmError> {
    args.pop().and_then(|v| v.as_bool()).ok_or(VmError::Malformed("expected bool argument to native call"))
}

fn throw_format_error(message: impl Into<String>) -> VmError {
    throw_native("NumberFormatException", message)
}

fn throw_native(class_name: &str, message: impl Into<String>) -> VmError {
    let mut fields = HashMap::new();
    fields.insert("message".to_string(), Value::Str(Rc::new(message.into())));
    VmError::Thrown(Value::Object(Rc::new(RefCell::new(crate::value::Object {
        class_name: class_name.to_string(),
        fields,
    }))))
}

pub fn is_native_generic_class(fqcn: &str) -> bool {
    fqcn.starts_with("system.List<") || fqcn.starts_with("system.Map<")
}

/// `Opcode::New` against a native generic class — see this module's doc
/// comment for the field layout. Both collections start out empty; a
/// `List<T>(T[] initial)` constructor call fills `__data__` afterwards via
/// `construct_generic`.
pub fn new_generic_object(fqcn: &str) -> Value {
    let mut fields = HashMap::new();
    if fqcn.starts_with("system.List<") {
        fields.insert("__data__".to_string(), Value::Array(Rc::new(RefCell::new(Vec::new()))));
    } else {
        fields.insert("__keys__".to_string(), Value::Array(Rc::new(RefCell::new(Vec::new()))));
        fields.insert("__values__".to_string(), Value::Array(Rc::new(RefCell::new(Vec::new()))));
    }
    Value::Object(Rc::new(RefCell::new(Object { class_name: fqcn.to_string(), fields })))
}

/// `Opcode::InvokeSpecial` on a native generic class's `<construct>`. Only
/// `system.List<T>(T[] initial)` does anything; `List()` and `Map()` leave
/// the empty fields `new_generic_object` already set up untouched.
pub fn construct_generic(receiver: &Value, fqcn: &str, mut args: Vec<Value>) -> Result<(), VmError> {
    if fqcn.starts_with("system.List<") {
        if let Some(Value::Array(initial)) = args.pop() {
            list_data(receiver)?.borrow_mut().extend(initial.borrow().iter().cloned());
        }
    }
    Ok(())
}

/// `Opcode::InvokeInstance` against a native generic class — dispatched by
/// the *receiver's* runtime class, same as `resolve_virtual` would for a
/// bytecode-backed class.
pub fn dispatch_instance(fqcn: &str, name: &str, receiver: &Value, args: Vec<Value>) -> Result<Option<Value>, VmError> {
    if fqcn.starts_with("system.List<") {
        dispatch_list(name, receiver, args)
    } else {
        dispatch_map(name, receiver, args)
    }
}

type ArrayRc = Rc<RefCell<Vec<Value>>>;

fn list_data(receiver: &Value) -> Result<ArrayRc, VmError> {
    let Value::Object(obj) = receiver else {
        return Err(VmError::Malformed("expected List receiver"));
    };
    match obj.borrow().fields.get("__data__") {
        Some(Value::Array(a)) => Ok(Rc::clone(a)),
        _ => Err(VmError::Malformed("malformed List object")),
    }
}

fn dispatch_list(name: &str, receiver: &Value, mut args: Vec<Value>) -> Result<Option<Value>, VmError> {
    let data = list_data(receiver)?;
    match name {
        "size" => Ok(Some(Value::Int(data.borrow().len() as i64))),
        "get" => {
            let idx = expect_int(&mut args)?;
            let d = data.borrow();
            if idx < 0 || idx as usize >= d.len() {
                return Err(throw_native("IndexOutOfBoundsException", format!("index {idx}, length {}", d.len())));
            }
            Ok(Some(d[idx as usize].clone()))
        }
        "set" => {
            let value = args.pop().ok_or(VmError::Malformed("missing value argument"))?;
            let idx = expect_int(&mut args)?;
            let mut d = data.borrow_mut();
            if idx < 0 || idx as usize >= d.len() {
                return Err(throw_native("IndexOutOfBoundsException", format!("index {idx}, length {}", d.len())));
            }
            d[idx as usize] = value;
            Ok(None)
        }
        "pushBack" | "add" => {
            let value = args.pop().ok_or(VmError::Malformed("missing value argument"))?;
            data.borrow_mut().push(value);
            Ok(None)
        }
        "pushFront" => {
            let value = args.pop().ok_or(VmError::Malformed("missing value argument"))?;
            data.borrow_mut().insert(0, value);
            Ok(None)
        }
        "popBack" => match data.borrow_mut().pop() {
            Some(v) => Ok(Some(v)),
            None => Err(throw_native("IndexOutOfBoundsException", "popBack on empty list")),
        },
        "popFront" => {
            let mut d = data.borrow_mut();
            if d.is_empty() {
                return Err(throw_native("IndexOutOfBoundsException", "popFront on empty list"));
            }
            Ok(Some(d.remove(0)))
        }
        "remove" => {
            let idx = expect_int(&mut args)?;
            let mut d = data.borrow_mut();
            if idx < 0 || idx as usize >= d.len() {
                return Err(throw_native("IndexOutOfBoundsException", format!("index {idx}, length {}", d.len())));
            }
            Ok(Some(d.remove(idx as usize)))
        }
        "contains" => {
            let value = args.pop().ok_or(VmError::Malformed("missing value argument"))?;
            Ok(Some(Value::Bool(data.borrow().iter().any(|v| values_equal(v, &value)))))
        }
        _ => Err(VmError::MethodNotFound(format!("system.List.{name}"))),
    }
}

fn map_storage(receiver: &Value) -> Result<(ArrayRc, ArrayRc), VmError> {
    let Value::Object(obj) = receiver else {
        return Err(VmError::Malformed("expected Map receiver"));
    };
    let obj = obj.borrow();
    match (obj.fields.get("__keys__"), obj.fields.get("__values__")) {
        (Some(Value::Array(k)), Some(Value::Array(v))) => Ok((Rc::clone(k), Rc::clone(v))),
        _ => Err(VmError::Malformed("malformed Map object")),
    }
}

fn dispatch_map(name: &str, receiver: &Value, mut args: Vec<Value>) -> Result<Option<Value>, VmError> {
    let (keys, values) = map_storage(receiver)?;
    match name {
        "size" => Ok(Some(Value::Int(keys.borrow().len() as i64))),
        "get" => {
            let key = args.pop().ok_or(VmError::Malformed("missing key argument"))?;
            let idx = keys.borrow().iter().position(|k| values_equal(k, &key));
            Ok(Some(match idx {
                Some(i) => values.borrow()[i].clone(),
                None => Value::Null,
            }))
        }
        "set" => {
            let value = args.pop().ok_or(VmError::Malformed("missing value argument"))?;
            let key = args.pop().ok_or(VmError::Malformed("missing key argument"))?;
            let idx = keys.borrow().iter().position(|k| values_equal(k, &key));
            match idx {
                Some(i) => values.borrow_mut()[i] = value,
                None => {
                    keys.borrow_mut().push(key);
                    values.borrow_mut().push(value);
                }
            }
            Ok(None)
        }
        "remove" => {
            let key = args.pop().ok_or(VmError::Malformed("missing key argument"))?;
            let idx = keys.borrow().iter().position(|k| values_equal(k, &key));
            match idx {
                Some(i) => {
                    keys.borrow_mut().remove(i);
                    values.borrow_mut().remove(i);
                    Ok(Some(Value::Bool(true)))
                }
                None => Ok(Some(Value::Bool(false))),
            }
        }
        "has" => {
            let key = args.pop().ok_or(VmError::Malformed("missing key argument"))?;
            Ok(Some(Value::Bool(keys.borrow().iter().any(|k| values_equal(k, &key)))))
        }
        "keys" => Ok(Some(Value::Array(Rc::new(RefCell::new(keys.borrow().clone()))))),
        "values" => Ok(Some(Value::Array(Rc::new(RefCell::new(values.borrow().clone()))))),
        _ => Err(VmError::MethodNotFound(format!("system.Map.{name}"))),
    }
}
