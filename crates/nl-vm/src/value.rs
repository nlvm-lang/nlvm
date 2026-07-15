use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

/// A heap-allocated class instance — see nlvm-specs/docs/vm.md § Object
/// layout. Fields are keyed by name rather than a declaration-order offset:
/// simpler than replicating the exact header/offset layout, and equivalent
/// as far as anything observable (no code inspects raw memory layout).
#[derive(Debug)]
pub struct Object {
    pub class_name: String,
    pub fields: HashMap<String, Value>,
}

/// Tagged runtime value — see nlvm-specs/docs/vm.md § Value representation.
///
/// Heap payloads use `Arc`/`Mutex` rather than `Rc`/`RefCell`: vm.md §
/// Threading model requires heap objects (arrays, objects — including
/// closures, which are just objects) to be shared across `system.thread.Thread`
/// instances, each of which is a real OS thread (see `crate::native`'s
/// thread section). `Rc`/`RefCell` are `!Send`/`!Sync`, so a `Value` could
/// never cross a `std::thread::spawn` boundary with them. `Mutex` also
/// gives every field/element access the memory-visibility guarantee
/// vm.md's threading model actually asks for from *synchronized* access
/// (the stdlib itself documents `List`/`Map` as "not thread-safe" — callers
/// must use `system.thread.Mutex` for their own invariants — but the VM's
/// own bookkeeping, e.g. two threads appending to the same array via
/// native calls, must still never be a Rust-level data race).
#[derive(Debug, Clone)]
pub enum Value {
    Null,
    Int(i64),
    Float(f64),
    Bool(bool),
    Byte(u8),
    Str(Arc<String>),
    Array(Arc<Mutex<Vec<Value>>>),
    Object(Arc<Mutex<Object>>),
}

/// Locks `m`, recovering from poisoning instead of panicking. A poisoned
/// `Mutex` (another thread panicked while holding it) would otherwise turn
/// every *other* thread's next access into a panic too — one buggy native
/// call would cascade into aborting unrelated threads. Recovering (taking
/// the guard anyway) matches vm.md's general stance that a single thread's
/// failure shouldn't take down the others; the poisoned data itself was
/// already left in a valid (if possibly stale) state by every mutation in
/// this codebase, none of which can panic mid-update.
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Value {
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Null => "null",
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::Bool(_) => "bool",
            Value::Byte(_) => "byte",
            Value::Str(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_float(&self) -> Option<f64> {
        match self {
            Value::Float(v) => Some(*v),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(v) => Some(*v),
            _ => None,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    pub fn to_display_string(&self) -> String {
        match self {
            Value::Null => "null".to_string(),
            Value::Int(v) => v.to_string(),
            Value::Float(v) => format!("{v}"),
            Value::Bool(v) => v.to_string(),
            Value::Byte(v) => v.to_string(),
            Value::Str(s) => (**s).clone(),
            Value::Array(_) => "[array]".to_string(),
            // Stringable dispatch (calling `toString()`) isn't implemented
            // this phase; nl-codegen never emits TO_STRING for an object
            // operand (string concatenation only accepts primitives/strings
            // — compiler.md's Stringable check is future work), so this is
            // an unreachable fallback, not a real code path.
            Value::Object(obj) => format!("[object {}]", lock(obj).class_name),
        }
    }
}
