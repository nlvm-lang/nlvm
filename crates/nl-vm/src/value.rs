use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use crate::program::Program;

/// A heap-allocated class instance — see nlvm-specs/docs/vm.md § Object
/// layout. Fields are keyed by name rather than a declaration-order offset:
/// simpler than replicating the exact header/offset layout, and equivalent
/// as far as anything observable (no code inspects raw memory layout).
///
/// The GC contract (vm.md § Garbage collection contract) is fulfilled by
/// reference counting: `Arc`'s refcount *is* the garbage collector — memory
/// is reclaimed exactly when the last reference drops, which also makes
/// destructor calls "prompt" (end of scope) as vm.md recommends. The known
/// refcounting limitation applies: objects in a reference cycle are never
/// reclaimed, so their destructors never run (conformant only for acyclic
/// object graphs; a cycle collector would be a later addition).
#[derive(Debug)]
pub struct Object {
    pub class_name: String,
    pub fields: HashMap<String, Value>,
    /// Back-reference to the running program, used by `Drop` to resolve
    /// `<destruct>`. `Weak` both to stay out of the refcount (a `Thread`'s
    /// closure captures `Arc<Program>` *and* objects, which would otherwise
    /// cycle) and because native/stdlib objects have no bytecode class to
    /// look a destructor up in — they use `Weak::new()` (never upgrades).
    pub(crate) program: Weak<Program>,
    /// "A destructor is called at most once per object" (vm.md): set on the
    /// resurrection copy `Drop` builds to run `<destruct>` on, so that copy
    /// — even if the destructor leaks `this` back into a live structure and
    /// it dies again later — never re-triggers the destructor.
    pub(crate) destroyed: bool,
}

impl Object {
    /// A native/stdlib instance (`system.*` handles, result records,
    /// VM-raised exceptions): no backing bytecode module, hence no
    /// destructor lookup on drop.
    pub fn native(class_name: impl Into<String>, fields: HashMap<String, Value>) -> Object {
        Object {
            class_name: class_name.into(),
            fields,
            program: Weak::new(),
            destroyed: false,
        }
    }
}

impl Drop for Object {
    /// GC hook — vm.md § Garbage collection contract: "If a class defines a
    /// `destruct` method, the VM must call it before reclaiming the
    /// object's memory." Runs when the last `Arc` to this object drops.
    /// `<destruct>` is resolved like any virtual method (walking `extends`,
    /// so an inherited destructor runs for subclasses too; only the most
    /// derived one runs — the spec defines no C++-style chaining). Since
    /// `Drop` only has `&mut self` (the `Arc` is already gone), the fields
    /// are moved into a fresh, pre-`destroyed` object to serve as `this`.
    /// A thrown exception is silently discarded per the same contract.
    fn drop(&mut self) {
        if self.destroyed {
            return;
        }
        let Some(program) = self.program.upgrade() else {
            return;
        };
        let Some((module, method)) = crate::interpreter::resolve_virtual(
            &program,
            &self.class_name,
            "<destruct>",
            "() -> void",
        ) else {
            return;
        };
        let this = Value::Object(Arc::new(Mutex::new(Object {
            class_name: std::mem::take(&mut self.class_name),
            fields: std::mem::take(&mut self.fields),
            program: std::mem::replace(&mut self.program, Weak::new()),
            destroyed: true,
        })));
        let _ = crate::interpreter::call_instance(&program, module, method, this, Vec::new());
    }
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
            // Plain fallback representation — no `Program` access here, so
            // no way to call back into a `Stringable`-implementing object's
            // `toString()`. `interpreter::display_string_of` is the real
            // `TO_STRING`-site entry point (it has `Program` and prefers
            // `toString()` when the runtime class implements `Stringable`);
            // this stays the fallback for everything else, e.g.
            // `program::describe_exception`'s non-`Object` case.
            Value::Object(obj) => format!("[object {}]", lock(obj).class_name),
        }
    }
}
