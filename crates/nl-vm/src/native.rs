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
//! parsing/formatting, and `system.String` (instance methods on `string`
//! values and their static equivalents — both compile to the same
//! `INVOKE_STATIC system.String.<name>`, see `nl_codegen::stdlib`). File
//! I/O, List/Map, threads, etc. are future work.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use crate::error::VmError;
use crate::program::Program;
use crate::value::Value;

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
