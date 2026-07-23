//! Cycle collector — closes the gap `value.rs`'s module doc calls out as
//! the "known refcounting limitation". Plain `Arc` reclaims any *acyclic*
//! object graph promptly (see `Object`'s `Drop` impl): the moment the last
//! strong reference to a node drops, its count hits zero and Rust's own
//! `Drop` glue runs. A group of objects/arrays that only reference each
//! other, with nothing external pointing into the group, never sees that
//! happen on its own — every member always has at least one referrer
//! (another member), so the count never reaches zero and `<destruct>`
//! never fires. This module adds a second, synchronous pass — run
//! *alongside*, never instead of, ordinary `Arc` refcounting — that
//! specifically looks for and reclaims exactly that situation.
//!
//! ## Why trial deletion, and why only from noted candidates
//!
//! This VM has no reified root set to trace a classic mark-sweep from:
//! `interpreter::run_frame`'s `locals`/operand `stack` are ordinary Rust
//! stack variables inside a recursively-called function, not entries in
//! any walkable table (`call_stack.rs`, the only "shadow stack" that
//! exists, tracks class/method/line for stack traces, not `Value`s). So
//! instead of tracing from roots, this is a Bacon-Rajan / CPython-`gc`
//! style **trial deletion**: it only ever looks at nodes explicitly noted
//! as *possible* cycle members — every point in `interpreter.rs`/
//! `program.rs` where a strong reference is dropped from a durable slot
//! (a field, an array element, a local variable, or a `static` field) and
//! might, but doesn't necessarily, leave its referent orphaned. From each
//! noted candidate, the algorithm:
//!
//! 1. Closes the set over outgoing edges (a cycle almost always spans more
//!    than one candidate).
//! 2. Computes, for every node in the closed set, its real `Arc` strong
//!    count minus one internal edge for every reference from another
//!    member of the same set — what's left is exactly how many references
//!    come from *outside* the set.
//! 3. Anything left with a positive count is reachable from outside the
//!    set (a real root, or a live object this pass didn't trace into);
//!    that liveness propagates to everything reachable from it.
//! 4. Whatever is never marked live is provably part of an unreachable
//!    cycle: its destructor (if any) runs and its outgoing edges are
//!    cleared, breaking the cycle so the normal `Arc`/`Drop` machinery
//!    reclaims the memory exactly as it would for acyclic garbage.
//!
//! ## Known limitations (documented, not fixed here)
//!
//! - **Not every reference drop is instrumented.** Only durable slots are
//!   (see above) — an operand popped off the evaluation stack (`POP`, or
//!   `stack.clear()` on exception unwinding) isn't, since forming a
//!   *cycle* requires an assignment into a durable slot in the first
//!   place. A candidate that survives a pass (still externally reachable
//!   at the time) is kept and re-checked on the next pass rather than
//!   discarded, so a cycle whose last root disappears via an
//!   uninstrumented event still gets collected once *any* later event
//!   triggers another pass — or, at the latest, `final_sweep` at program
//!   exit — without needing every possible drop site instrumented.
//! - **Reassigning a field/local out of a cycle doesn't always make it
//!   collectible *immediately*.** `nl_codegen::expr::compile_assign` treats
//!   assignment as an expression (`a = b = 1;`), so `obj.field = x;` also
//!   leaves a copy of `x` behind — in a compiler-generated scratch local
//!   for a field/static target, on the operand stack (until the very next
//!   `POP`) for a plain local target — even when the assignment is used as
//!   a bare statement. That extra copy is itself a durable reference (for
//!   the scratch-local case) that keeps the old cycle externally reachable
//!   until the scratch slot is reused by a later assignment or the frame
//!   returns. Collection still happens — just not necessarily before the
//!   *very next* statement — which is within what vm.md's "should call
//!   destructors promptly... not required to do so immediately" allows.
//! - **Not safe against concurrent structural mutation of the same
//!   objects from another thread while a pass is running.** Each pass
//!   reads `Arc::strong_count`/fields as a series of snapshots, not one
//!   atomic snapshot; a `system.thread.Thread` concurrently rewriting the
//!   same object graph mid-pass could in principle make a live object
//!   look momentarily unreachable. This mirrors the rest of the VM's
//!   threading story (stdlib `Map`/`List` are documented as "not
//!   thread-safe" without the caller's own `system.thread.Mutex`) rather
//!   than a new kind of risk — programs that share one object graph
//!   across threads without their own synchronization already have no
//!   other consistency guarantees from this VM.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, Weak};

use crate::program::Program;
use crate::value::{lock, Object, Value};

/// A heap container that can hold further `Value`s and thus form part of a
/// reference cycle — the only two `Value` variants backed by shared
/// interior mutability. `Value::Str` is an immutable `Arc<String>` with no
/// outgoing edges, so a string can never participate in a cycle and is
/// never turned into a `GcNode`.
pub(crate) enum GcNode {
    Object(Weak<Mutex<Object>>),
    Array(Weak<Mutex<Vec<Value>>>),
}

/// Same as `GcNode`, but holding a strong reference — what the collector
/// actually works with once a candidate is confirmed still alive.
enum GcHeld {
    Object(Arc<Mutex<Object>>),
    Array(Arc<Mutex<Vec<Value>>>),
}

impl GcHeld {
    /// Stable identity for this node, usable as a hash-map key: two
    /// `GcHeld`s reached via different paths through the graph but
    /// pointing at the same allocation must collapse to one entry.
    fn ptr_key(&self) -> usize {
        match self {
            GcHeld::Object(a) => Arc::as_ptr(a) as usize,
            GcHeld::Array(a) => Arc::as_ptr(a) as usize,
        }
    }

    fn strong_count(&self) -> i64 {
        (match self {
            GcHeld::Object(a) => Arc::strong_count(a),
            GcHeld::Array(a) => Arc::strong_count(a),
        }) as i64
    }

    /// Every `Value` this node directly holds a strong reference to —
    /// this node's outgoing edges in the object graph. Locks just long
    /// enough to clone the field/element values (cheap `Arc` bumps), same
    /// discipline as `SET_FIELD`/`ARRAY_STORE`: never hold the lock any
    /// longer than that.
    fn outgoing(&self) -> Vec<Value> {
        match self {
            GcHeld::Object(a) => lock(a).fields.values().cloned().collect(),
            GcHeld::Array(a) => lock(a).clone(),
        }
    }

    fn downgrade(&self) -> GcNode {
        match self {
            GcHeld::Object(a) => GcNode::Object(Arc::downgrade(a)),
            GcHeld::Array(a) => GcNode::Array(Arc::downgrade(a)),
        }
    }
}

fn key_of(value: &Value) -> Option<usize> {
    match value {
        Value::Object(a) => Some(Arc::as_ptr(a) as usize),
        Value::Array(a) => Some(Arc::as_ptr(a) as usize),
        _ => None,
    }
}

fn held_of(value: &Value) -> Option<GcHeld> {
    match value {
        Value::Object(a) => Some(GcHeld::Object(a.clone())),
        Value::Array(a) => Some(GcHeld::Array(a.clone())),
        _ => None,
    }
}

fn upgrade(node: &GcNode) -> Option<GcHeld> {
    match node {
        GcNode::Object(w) => w.upgrade().map(GcHeld::Object),
        GcNode::Array(w) => w.upgrade().map(GcHeld::Array),
    }
}

fn push_candidate(program: &Arc<Program>, value: &Value) {
    let node = match value {
        Value::Object(a) => GcNode::Object(Arc::downgrade(a)),
        Value::Array(a) => GcNode::Array(Arc::downgrade(a)),
        _ => return,
    };
    lock(&program.gc_pending).push(node);
}

/// Drains the pending-candidate buffer and, if it's non-empty, runs one
/// trial-deletion pass, then restores whatever survived (still externally
/// reachable) for a future pass to re-check.
fn run_pending_collection(program: &Arc<Program>) {
    let candidates = std::mem::take(&mut *lock(&program.gc_pending));
    if candidates.is_empty() {
        return;
    }
    let survivors = collect_cycles(candidates);
    lock(&program.gc_pending).extend(survivors);
}

/// Registers `value` as a possible cycle-collector root and immediately
/// runs a pass. Called at every point in `interpreter.rs`/`program.rs`
/// where a strong reference to an `Object`/`Array` is dropped from a
/// durable slot (see this module's doc comment for which ones and why).
///
/// Takes `value` by ownership — not just to note it, but so this function
/// can drop *this specific reference* before checking any strong counts.
/// The caller (e.g. `SET_FIELD`) has already removed `value` from the slot
/// it used to occupy, but the local Rust binding holding it is still a
/// live reference in its own right; if the trial-deletion pass ran while
/// that binding were still alive, every node it (transitively) points to
/// would look artificially one reference count too high — masking the
/// exact instant a cycle actually becomes unreachable, which is the one
/// case this collector exists to catch. Callers must drop their *own*
/// other temporaries (e.g. `SET_FIELD`'s receiver) before calling this for
/// the same reason — see the call sites in `interpreter.rs`.
///
/// Runs eagerly, with no batching or threshold: this VM's ordinary `Arc`
/// path already reclaims acyclic garbage the instant a count hits zero
/// (vm.md recommends "prompt" destructor calls), so doing the same here
/// keeps cyclic and acyclic destructor timing equally predictable —
/// useful given fixture tests assert exact `stdout` ordering.
pub(crate) fn note_and_collect(program: &Arc<Program>, value: Value) {
    push_candidate(program, &value);
    drop(value);
    run_pending_collection(program);
}

/// Bulk counterpart to `note_and_collect` for `interpreter::run_frame`'s
/// exit points: every local in a returning frame becomes a candidate at
/// once (locals live for the whole frame in this VM, not per source-level
/// block — see `run_frame`'s doc comment — so a frame returning is the
/// only place a local going out of scope is ever observable), collected
/// in one pass rather than one pass per slot.
///
/// Takes `locals` by ownership for the same reason `note_and_collect`
/// takes `value` by ownership: `run_frame`'s own `locals: Vec<Value>`
/// binding is still alive (and still holding a strong reference to every
/// local in it) right up until the point it actually returns, so it must
/// be dropped here, before checking any strong counts — otherwise every
/// local would look externally referenced by the very frame that's in
/// the middle of returning.
pub(crate) fn note_locals_and_collect(program: &Arc<Program>, locals: Vec<Value>) {
    for v in &locals {
        push_candidate(program, v);
    }
    drop(locals);
    run_pending_collection(program);
}

/// Final chance for whatever's left in the pending buffer, called once as
/// `run_program` is about to capture `stdout`/`stderr` — see that call
/// site's comment on why the result value is dropped first. Covers a
/// cycle whose last root disappeared via an uninstrumented event (this
/// module's doc comment) with nothing left afterward to trigger another
/// pass on its own.
pub(crate) fn final_sweep(program: &Arc<Program>) {
    run_pending_collection(program);
}

/// The trial-deletion pass itself — see this module's doc comment for the
/// algorithm. Returns whatever candidates turned out to still be
/// externally reachable, so the caller can keep watching them: a survivor
/// today may become garbage later without necessarily being re-noted
/// (e.g. its last root leaves via an uninstrumented `POP`).
fn collect_cycles(candidates: Vec<GcNode>) -> Vec<GcNode> {
    // Close the candidate set over outgoing edges: some cycle members may
    // only be reachable *through* a candidate rather than having been
    // noted directly themselves. `edges` caches each node's outgoing
    // *identities* (`usize` keys) — not `Value` clones — precisely so this
    // bookkeeping never itself holds an extra strong reference: a cached
    // `Value` would keep its `Arc` alive for as long as `edges` lives,
    // silently inflating the very strong counts the subtraction pass below
    // depends on being accurate.
    let mut set: HashMap<usize, GcHeld> = HashMap::new();
    let mut edges: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut worklist: Vec<usize> = Vec::new();

    for node in candidates {
        if let Some(held) = upgrade(&node) {
            let key = held.ptr_key();
            if let std::collections::hash_map::Entry::Vacant(e) = set.entry(key) {
                e.insert(held);
                worklist.push(key);
            }
        }
    }

    while let Some(key) = worklist.pop() {
        // `out`'s `Value` clones (and the extra `held_of` clone for a
        // newly-discovered node) are intentionally transient: both drop by
        // the end of this loop iteration, leaving only `out_keys` (plain
        // identities) and, for genuinely new nodes, one clone owned by
        // `set` itself — already accounted for by the `- 1` below.
        let out = set[&key].outgoing();
        let mut out_keys = Vec::with_capacity(out.len());
        for v in &out {
            if let Some(k) = key_of(v) {
                out_keys.push(k);
                if let std::collections::hash_map::Entry::Vacant(e) = set.entry(k) {
                    if let Some(held) = held_of(v) {
                        e.insert(held);
                        worklist.push(k);
                    }
                }
            }
        }
        edges.insert(key, out_keys);
    }

    if set.is_empty() {
        return Vec::new();
    }

    // Trial subtraction: each node's real strong count, minus the one
    // clone `set` itself now holds (an artifact of this pass, not a real
    // reference), minus one for every internal edge pointing at it. What
    // remains is exactly how many references to it come from *outside*
    // this closed set.
    let mut external: HashMap<usize, i64> = set
        .iter()
        .map(|(k, held)| (*k, held.strong_count() - 1))
        .collect();
    for targets in edges.values() {
        for tk in targets {
            if let Some(count) = external.get_mut(tk) {
                *count -= 1;
            }
        }
    }

    // Anything left with a positive count is reachable from outside the
    // set — a genuine root, or a live object this pass simply never
    // traced into. It, and everything reachable from it, is live;
    // propagate that from every such node along outgoing edges.
    let mut live: HashSet<usize> = HashSet::new();
    let mut stack: Vec<usize> = external
        .iter()
        .filter(|(_, c)| **c > 0)
        .map(|(k, _)| *k)
        .collect();
    while let Some(key) = stack.pop() {
        if !live.insert(key) {
            continue;
        }
        if let Some(targets) = edges.get(&key) {
            for k in targets {
                if set.contains_key(k) && !live.contains(k) {
                    stack.push(*k);
                }
            }
        }
    }

    let mut survivors = Vec::new();
    let mut garbage: Vec<GcHeld> = Vec::new();
    for (key, held) in set {
        if live.contains(&key) {
            survivors.push(held.downgrade());
        } else {
            garbage.push(held);
        }
    }

    // Run every garbage object's destructor first, with fields/elements
    // still intact at the moment each one runs (vm.md § Garbage
    // collection contract: "must call it before reclaiming the object's
    // memory"), *then* clear arrays. By the time every internal edge in
    // this batch has been severed, the only strong references left are
    // the ones `garbage` itself holds, so dropping it brings every count
    // in the batch down to zero and hands off to `Object`'s own `Drop`
    // impl — a no-op there, since `force_destroy` already marked it
    // destroyed.
    for held in &garbage {
        if let GcHeld::Object(arc) = held {
            force_destroy(arc);
        }
    }
    for held in &garbage {
        if let GcHeld::Array(arc) = held {
            lock(arc).clear();
        }
    }
    drop(garbage);

    survivors
}

/// Cycle-collector counterpart to `Object`'s own `Drop` impl (`value.rs`).
/// A natural `Drop` only ever fires once the `Arc` strong count has
/// genuinely reached zero, at which point Rust hands `drop` direct
/// `&mut Object` access with no `Mutex` involved at all — nothing else can
/// possibly be looking at the data anymore. A cycle's members never reach
/// zero on their own, so this forces the same "run `<destruct>` at most
/// once, then let the fields go" step early, while other strong
/// references within the very same (about-to-be-collected) batch still
/// exist — which means it must go through a real `lock()`, and that lock
/// must be released *before* calling the destructor: the same rule
/// `SET_FIELD`/`ARRAY_STORE` already follow, since a destructor calling
/// back into another not-yet-processed member of the same cycle while
/// this object's own lock is still held would deadlock (`std::sync::
/// Mutex` is not reentrant).
///
/// Always takes `fields`/`class_name` — breaking this object's outgoing
/// edges — even when there's no destructor to run or no live `Program` to
/// resolve one against (a native/stdlib object). The collector depends on
/// every garbage node's edges being severed unconditionally; `Object::
/// drop` can skip that work because `self` is about to be deallocated in
/// full regardless, but here the object is *not* about to be deallocated
/// by this call alone — dropping `garbage`'s own strong clone afterward is
/// what finally does that.
fn force_destroy(arc: &Arc<Mutex<Object>>) {
    let (maybe_program, class_name, this) = {
        let mut guard = lock(arc);
        if guard.destroyed {
            return;
        }
        guard.destroyed = true;
        let maybe_program = guard.program.upgrade();
        let class_name = guard.class_name.clone();
        let this = Value::Object(Arc::new(Mutex::new(Object {
            class_name: std::mem::take(&mut guard.class_name),
            fields: std::mem::take(&mut guard.fields),
            program: std::mem::replace(&mut guard.program, Weak::new()),
            destroyed: true,
        })));
        (maybe_program, class_name, this)
    };
    let Some(program) = maybe_program else {
        return;
    };
    if let Some((module, method)) =
        crate::interpreter::resolve_virtual(&program, &class_name, "<destruct>", "() -> void")
    {
        let _ = crate::interpreter::call_instance(&program, module, method, this, Vec::new());
    }
}
