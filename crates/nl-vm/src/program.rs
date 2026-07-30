use std::cell::RefCell;
use std::collections::HashMap;
use std::io::BufRead;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use nl_bytecode::{class_flags, field_flags, method_flags, MethodDescriptor, Module};

use crate::error::VmError;
use crate::interpreter::{call_static, default_value_for};
use crate::value::{lock, Value};

/// A counting synchronization primitive shared by `system.thread.Mutex`
/// (as a 0/1 lock: `bool` doubles as "locked") and `system.thread.Semaphore`
/// (as a bounded counter). Built on `Condvar` rather than holding a
/// `MutexGuard` across the `lock()`/`unlock()` call boundary — a guard
/// can't outlive the single native call that acquires it, but the *logical*
/// lock must stay held across arbitrarily many other native calls in
/// between (vm.md § Threading model's mutex happens-before guarantee is
/// about `lock()`/`unlock()` call pairs, not Rust's own borrow scopes).
pub(crate) struct Counter {
    state: Mutex<i64>,
    condvar: Condvar,
}

impl Counter {
    fn new(initial: i64) -> Arc<Counter> {
        Arc::new(Counter {
            state: Mutex::new(initial),
            condvar: Condvar::new(),
        })
    }

    /// Blocks while the count is `0`, then decrements it by one.
    pub(crate) fn acquire(&self) {
        let mut guard = lock(&self.state);
        while *guard == 0 {
            guard = self.condvar.wait(guard).unwrap_or_else(|e| e.into_inner());
        }
        *guard -= 1;
    }

    pub(crate) fn try_acquire(&self) -> bool {
        let mut guard = lock(&self.state);
        if *guard == 0 {
            false
        } else {
            *guard -= 1;
            true
        }
    }

    pub(crate) fn release(&self) {
        let mut guard = lock(&self.state);
        *guard += 1;
        self.condvar.notify_one();
    }
}

/// One `system.thread.Thread`'s bookkeeping. The slot is allocated by
/// `construct` (not `start()`) because `interrupt()` must have somewhere to
/// write to on a thread that hasn't started yet.
struct ThreadSlot {
    /// `None` before `start()`, and again after a completed `join()` (taken
    /// out by `join_thread` — a slot left handle-less that way reads back as
    /// "already joined", matching `FileHandle`'s close-is-terminal pattern).
    handle: Option<JoinHandle<()>>,
    /// The OS thread `start()` spawned, kept separately from `handle` so
    /// `interrupt()` can still `unpark()` a thread whose `JoinHandle` was
    /// already taken (harmless: unparking a finished thread is a no-op).
    os_thread: Option<std::thread::Thread>,
    /// False until `start()`; distinguishes "never started" (`isAlive()` is
    /// false, `join()` is a no-op) from "started and finished".
    started: bool,
    /// Interrupt flag — stdlib.md § system.thread.Thread's
    /// `InterruptedException`. Set by `interrupt()` from any thread, cleared
    /// by whichever interruptible wait (`join`/`join(timeout)`/`sleep`) acts
    /// on it. Shared with the spawned thread itself, which installs it as
    /// its `CURRENT_INTERRUPT` so `Thread.sleep` — a *static* call with no
    /// receiver to consult — knows which flag is its own.
    interrupt: Arc<AtomicBool>,
}

thread_local! {
    /// Interrupt flag of the NL thread running on this OS thread, installed
    /// by `set_current_interrupt` at the top of every spawned thread's task.
    /// Stays `None` on the main thread, which is deliberate: nothing in the
    /// spec designates the main thread (no `Thread` object refers to it), so
    /// it can never be interrupted — and a `None` flag is what lets
    /// `join`/`sleep` there keep using a plain blocking wait instead of the
    /// interruptible polling loop (see `crate::native`'s thread section).
    static CURRENT_INTERRUPT: RefCell<Option<Arc<AtomicBool>>> =
        const { RefCell::new(None) };
}

/// Called once, by a spawned thread, before it runs its task.
pub(crate) fn set_current_interrupt(flag: Arc<AtomicBool>) {
    CURRENT_INTERRUPT.with(|slot| *slot.borrow_mut() = Some(flag));
}

/// The calling thread's interrupt flag, or `None` on a thread that can't be
/// interrupted at all (the main thread — see `CURRENT_INTERRUPT`).
pub(crate) fn current_interrupt() -> Option<Arc<AtomicBool>> {
    CURRENT_INTERRUPT.with(|slot| slot.borrow().clone())
}

/// Tests *and clears* the calling thread's interrupt flag: true means an
/// `interrupt()` was pending, and the caller now owes an
/// `InterruptedException`. Clearing on delivery mirrors the exception being
/// consumed — a second wait doesn't throw again unless interrupted anew.
pub(crate) fn take_interrupt(flag: &AtomicBool) -> bool {
    flag.swap(false, Ordering::SeqCst)
}

/// A linked program: every module that will be executed together, keyed by
/// fully-qualified class name. Built once per run so cross-file references
/// (`new`, field access, instance/static method calls — see
/// `nl_bytecode::ConstantPoolEntry::{Class,FieldRef,MethodRef}`) resolve to
/// the right module instead of assuming everything lives in one file.
///
/// Wrapped in `Arc` by every entry point (`run_program`, `native::Thread`'s
/// `start()`) rather than borrowed: a spawned `system.thread.Thread` runs
/// on a real OS thread (`std::thread::spawn`, which requires `'static`
/// captures), so it needs to *own* a handle to the program, not merely
/// borrow one tied to the spawning frame's stack.
pub struct Program {
    /// Every module, in the order `Program::new` received them (the order
    /// `nl_codegen::compile_program` emitted them in — prelude first, then
    /// each source file). Indexed rather than keyed by name so a vtable
    /// slot can point at its method with two array indices and no hashing
    /// (see `VTableEntry`), and so `run_static_initializers` gets a
    /// deterministic, reproducible `<clinit>` sequence for free.
    modules: Vec<Module>,
    /// FQCN → index into `modules`, for everything that resolves a class by
    /// name (`NEW`, `GET_STATIC`, `INSTANCEOF`, ...).
    by_name: HashMap<String, usize>,
    /// Precomputed method tables — vm.md § Method dispatch. Built once at
    /// link time by `verify_link`; see `VTables`.
    vtables: VTables,
    /// Per-class `static` field storage — specs.md § Classes. Keyed by
    /// declaring-class FQCN (never a subclass's, even when a field is
    /// referenced through one — see `nl_codegen::class_table::
    /// find_field_owner`), then field name. Pre-populated with every static
    /// field's type default at construction time; `run_static_initializers`
    /// overwrites the ones with a declared initializer before `main` runs.
    /// Enum case constants are never stored here (nl-codegen recompiles them
    /// at each use site instead of emitting `GET_STATIC`/`SET_STATIC`).
    statics: Mutex<HashMap<String, HashMap<String, Value>>>,
    /// Accumulated output from native `system.Out`/`system.Err` calls (see
    /// `crate::native`) — `Program` is shared across every call frame *and*
    /// every thread, so these are interior-mutable rather than threaded
    /// explicitly through `call_static`/`call_instance`/`run_frame`.
    stdout: Mutex<String>,
    stderr: Mutex<String>,
    /// Source for `system.In.readLine` (see `crate::native`). The real
    /// process stdin by default (`run_program`); `run_program_with_stdin`
    /// substitutes an in-memory buffer instead, which is what lets
    /// `nl-test-runner` script `system.In.readLine` in a YAML fixture
    /// without a real pipe (see `Header::stdin`) — the previous state was
    /// that `native::dispatch` called `std::io::stdin()` directly, which
    /// made `readLine` untestable in-process (nlvm issue #6).
    stdin: Mutex<Box<dyn BufRead + Send>>,
    /// Open files backing `system.io.FileHandle` objects (see
    /// `crate::native`): a handle object only carries an index into this
    /// table, and `close()` clears the slot (making the index permanently
    /// dead — stdlib.md: "After the handle has been closed, any call to
    /// read, readLine, write, or flush throws IOException").
    file_handles: Mutex<Vec<Option<std::fs::File>>>,
    /// Same pattern as `file_handles`, one table per `system.net.*` handle
    /// class (see `crate::native`'s network section). Kept as three
    /// separate tables rather than one enum table since each handle class
    /// only ever indexes its own.
    tcp_listeners: Mutex<Vec<Option<std::net::TcpListener>>>,
    tcp_streams: Mutex<Vec<Option<std::net::TcpStream>>>,
    udp_sockets: Mutex<Vec<Option<std::net::UdpSocket>>>,
    /// Backing store for `system.thread.Thread` — a thread object only
    /// carries an index into this table (`"__tid__"`, allocated by
    /// `construct`, since `interrupt()` needs a flag to set even before
    /// `start()`). See `ThreadSlot`.
    threads: Mutex<Vec<ThreadSlot>>,
    /// Backing store for `system.thread.Mutex` (`"__mid__"`) — modeled as a
    /// `Counter` capped at 1 (`lock`/`unlock`/`tryLock` treat `0` as locked,
    /// `1` as unlocked).
    thread_mutexes: Mutex<Vec<Option<Arc<Counter>>>>,
    /// Backing store for `system.thread.Semaphore` (`"__sid__"`).
    thread_semaphores: Mutex<Vec<Option<Arc<Counter>>>>,
    /// Cycle-collector candidate buffer — see `crate::gc`. Holds `Weak`
    /// handles to `Object`/`Array` nodes noted at every point a strong
    /// reference is dropped from a durable slot (field, array element,
    /// local variable, `static` field) or from the operand stack (`POP`,
    /// exception unwinding) without necessarily freeing the referent;
    /// `crate::gc::collect_cycles` drains and re-populates it with whatever
    /// survives each pass.
    pub(crate) gc_pending: Mutex<Vec<crate::gc::GcNode>>,
    /// Deferred candidates noted since the last collector pass — see
    /// `crate::gc::note_discarded`. Operand-stack drops are far too frequent
    /// to run a pass on each one, so they only trigger one once this
    /// counter reaches `gc::DEFERRED_PASS_THRESHOLD`.
    pub(crate) gc_deferred: AtomicUsize,
}

impl Program {
    /// `stdin_data` is `None` to read the real process stdin (the
    /// `run_program` entry point), or `Some(bytes)` to serve `readLine`
    /// calls from an in-memory script instead (`run_program_with_stdin`).
    ///
    /// `vtables` comes from `verify_link`, which is the only thing that has
    /// the whole program in view at once; it must have been built from
    /// exactly this `modules` vector, in this order (its slots index into
    /// it).
    pub fn new(modules: Vec<Module>, vtables: VTables, stdin_data: Option<Vec<u8>>) -> Self {
        let mut by_name = HashMap::with_capacity(modules.len());
        let mut statics: HashMap<String, HashMap<String, Value>> = HashMap::new();
        for (index, module) in modules.iter().enumerate() {
            if let Some(name) = module.this_class_name() {
                let mut class_statics = HashMap::new();
                for f in &module.fields {
                    if f.flags & field_flags::STATIC == 0 {
                        continue;
                    }
                    let Some(field_name) = module.constant_pool.utf8_at(f.name_index) else {
                        continue;
                    };
                    let type_desc = module
                        .constant_pool
                        .type_desc_at(f.type_index)
                        .unwrap_or("void");
                    class_statics.insert(field_name.to_string(), default_value_for(type_desc));
                }
                statics.insert(name.to_string(), class_statics);
                by_name.insert(name.to_string(), index);
            }
        }
        let stdin: Box<dyn BufRead + Send> = match stdin_data {
            Some(bytes) => Box::new(std::io::Cursor::new(bytes)),
            None => Box::new(std::io::BufReader::new(std::io::stdin())),
        };
        Program {
            modules,
            by_name,
            vtables,
            statics: Mutex::new(statics),
            stdout: Mutex::new(String::new()),
            stderr: Mutex::new(String::new()),
            stdin: Mutex::new(stdin),
            file_handles: Mutex::new(Vec::new()),
            tcp_listeners: Mutex::new(Vec::new()),
            tcp_streams: Mutex::new(Vec::new()),
            udp_sockets: Mutex::new(Vec::new()),
            threads: Mutex::new(Vec::new()),
            thread_mutexes: Mutex::new(Vec::new()),
            thread_semaphores: Mutex::new(Vec::new()),
            gc_pending: Mutex::new(Vec::new()),
            gc_deferred: AtomicUsize::new(0),
        }
    }

    pub fn get(&self, fqcn: &str) -> Option<&Module> {
        self.modules.get(*self.by_name.get(fqcn)?)
    }

    pub fn find_main(&self) -> Option<(&Module, &MethodDescriptor)> {
        self.modules
            .iter()
            .find_map(|m| m.find_method("main").map(|meth| (m, meth)))
    }

    /// vm.md § Method dispatch: the vtable slot of `class_fqcn` matching
    /// `name` + the full descriptor. `class_fqcn` is the receiver's runtime
    /// class for `INVOKE_INSTANCE`/`INVOKE_CLOSURE`, or the class named in
    /// the method ref for `INVOKE_SPECIAL` — either way the slot already
    /// accounts for inheritance, so callers never walk `extends` themselves.
    /// `None` for a class with no module at all (every native class — see
    /// `crate::native`), same as the pre-vtable chain walk returned.
    pub(crate) fn resolve_method(
        &self,
        class_fqcn: &str,
        name: &str,
        descriptor: &str,
    ) -> Option<(&Module, &MethodDescriptor)> {
        let slots = self.vtables.classes.get(class_fqcn)?.get(name)?;
        self.method_at(slots.iter().find(|slot| slot.descriptor == descriptor)?)
    }

    /// Like `resolve_method`, but matches the parameter portion of the
    /// descriptor only, ignoring the return type — see
    /// `nl_bytecode::Module::find_method_by_name_and_params` for why that
    /// is the right match for an interface-typed receiver.
    pub(crate) fn resolve_method_covariant(
        &self,
        class_fqcn: &str,
        name: &str,
        descriptor: &str,
    ) -> Option<(&Module, &MethodDescriptor)> {
        let params_desc = descriptor.split(" -> ").next().unwrap_or(descriptor);
        let slots = self.vtables.classes.get(class_fqcn)?.get(name)?;
        self.method_at(slots.iter().find(|slot| slot.params() == params_desc)?)
    }

    /// Like `resolve_method`, but matches on name alone — for a native
    /// caller invoking a well-known single-overload method (see
    /// `crate::interpreter::resolve_virtual_by_name`). Slots are ordered
    /// nearest-declaration-first, so the first one is the same overload a
    /// chain walk would have stopped at.
    pub(crate) fn resolve_method_by_name(
        &self,
        class_fqcn: &str,
        name: &str,
    ) -> Option<(&Module, &MethodDescriptor)> {
        self.method_at(self.vtables.classes.get(class_fqcn)?.get(name)?.first()?)
    }

    fn method_at(&self, slot: &VTableEntry) -> Option<(&Module, &MethodDescriptor)> {
        let module = self.modules.get(slot.module_index)?;
        Some((module, module.methods.get(slot.method_index)?))
    }

    /// `GET_STATIC` — see `Opcode::GetStatic`'s doc comment in
    /// `crate::interpreter`. `None` means the constant-pool `FieldRef`
    /// named a class/field this table never saw a `static` declaration for
    /// (an nl-codegen bug, since every static field is pre-populated by
    /// `Program::new`), not "field currently unset".
    pub(crate) fn get_static(&self, class_fqcn: &str, field_name: &str) -> Option<Value> {
        lock(&self.statics)
            .get(class_fqcn)?
            .get(field_name)
            .cloned()
    }

    /// `SET_STATIC`. Silently a no-op for an unknown class/field, like
    /// `get_static`'s `None` case — never expected in practice, but there's
    /// no sensible value to store it under. Returns the value it replaced
    /// (always `Some` in practice, since every static field is pre-populated
    /// with a type default at construction) so the caller can hand it to
    /// `crate::gc::note_and_collect` — a `static` field is a durable slot
    /// just like an instance field, and can just as well be the last
    /// reference keeping a cycle's candidacy alive.
    pub(crate) fn set_static(
        &self,
        class_fqcn: &str,
        field_name: &str,
        value: Value,
    ) -> Option<Value> {
        lock(&self.statics)
            .get_mut(class_fqcn)?
            .insert(field_name.to_string(), value)
    }

    pub fn write_stdout(&self, s: &str) {
        lock(&self.stdout).push_str(s);
    }

    pub fn write_stderr(&self, s: &str) {
        lock(&self.stderr).push_str(s);
    }

    /// `system.In.readLine` (stdlib.md): one line from `stdin`, CRLF/LF
    /// trailing newline stripped, `None` on EOF with nothing read.
    pub fn read_stdin_line(&self) -> std::io::Result<Option<String>> {
        let mut line = String::new();
        if lock(&self.stdin).read_line(&mut line)? == 0 {
            return Ok(None);
        }
        if line.ends_with('\n') {
            line.pop();
            if line.ends_with('\r') {
                line.pop();
            }
        }
        Ok(Some(line))
    }

    pub fn register_file(&self, file: std::fs::File) -> i64 {
        let mut handles = lock(&self.file_handles);
        handles.push(Some(file));
        (handles.len() - 1) as i64
    }

    /// Idempotent, like `FileHandle.close()` itself (stdlib.md) — closing an
    /// already-closed or unknown id is a no-op. Dropping the `File` closes it.
    pub fn close_file(&self, id: i64) {
        if let Some(slot) = lock(&self.file_handles).get_mut(id as usize) {
            *slot = None;
        }
    }

    /// Runs `f` on the open file for `id`, or `None` if the id is unknown
    /// or the handle was closed (the caller turns that into `IOException`).
    pub fn with_file<R>(&self, id: i64, f: impl FnOnce(&mut std::fs::File) -> R) -> Option<R> {
        let mut handles = lock(&self.file_handles);
        handles.get_mut(id as usize)?.as_mut().map(f)
    }

    pub fn register_tcp_listener(&self, listener: std::net::TcpListener) -> i64 {
        let mut listeners = lock(&self.tcp_listeners);
        listeners.push(Some(listener));
        (listeners.len() - 1) as i64
    }

    pub fn close_tcp_listener(&self, id: i64) {
        if let Some(slot) = lock(&self.tcp_listeners).get_mut(id as usize) {
            *slot = None;
        }
    }

    pub fn with_tcp_listener<R>(
        &self,
        id: i64,
        f: impl FnOnce(&mut std::net::TcpListener) -> R,
    ) -> Option<R> {
        let mut listeners = lock(&self.tcp_listeners);
        listeners.get_mut(id as usize)?.as_mut().map(f)
    }

    pub fn register_tcp_stream(&self, stream: std::net::TcpStream) -> i64 {
        let mut streams = lock(&self.tcp_streams);
        streams.push(Some(stream));
        (streams.len() - 1) as i64
    }

    pub fn close_tcp_stream(&self, id: i64) {
        if let Some(slot) = lock(&self.tcp_streams).get_mut(id as usize) {
            *slot = None;
        }
    }

    pub fn with_tcp_stream<R>(
        &self,
        id: i64,
        f: impl FnOnce(&mut std::net::TcpStream) -> R,
    ) -> Option<R> {
        let mut streams = lock(&self.tcp_streams);
        streams.get_mut(id as usize)?.as_mut().map(f)
    }

    pub fn register_udp_socket(&self, socket: std::net::UdpSocket) -> i64 {
        let mut sockets = lock(&self.udp_sockets);
        sockets.push(Some(socket));
        (sockets.len() - 1) as i64
    }

    pub fn close_udp_socket(&self, id: i64) {
        if let Some(slot) = lock(&self.udp_sockets).get_mut(id as usize) {
            *slot = None;
        }
    }

    pub fn with_udp_socket<R>(
        &self,
        id: i64,
        f: impl FnOnce(&mut std::net::UdpSocket) -> R,
    ) -> Option<R> {
        let mut sockets = lock(&self.udp_sockets);
        sockets.get_mut(id as usize)?.as_mut().map(f)
    }

    /// `UdpSocket.bind(host, port)` re-binds the *same* handle to a chosen
    /// address — `construct()` already gave it an OS socket (an ephemeral
    /// port, so `send()` works without an explicit `bind()`), and `std`
    /// has no in-place rebind, so this swaps the slot for a freshly bound
    /// socket instead of allocating a new id/object.
    pub fn rebind_udp_socket(&self, id: i64, socket: std::net::UdpSocket) {
        if let Some(slot) = lock(&self.udp_sockets).get_mut(id as usize) {
            *slot = Some(socket);
        }
    }

    /// Allocates an unstarted slot for a freshly constructed `Thread` and
    /// returns its `__tid__`.
    pub(crate) fn register_thread(&self) -> i64 {
        let mut threads = lock(&self.threads);
        threads.push(ThreadSlot {
            handle: None,
            os_thread: None,
            started: false,
            interrupt: Arc::new(AtomicBool::new(false)),
        });
        (threads.len() - 1) as i64
    }

    /// The flag `start()` hands to the thread it is about to spawn (so the
    /// thread can install it as its own `CURRENT_INTERRUPT`), and that
    /// `interrupt()`/`isInterrupted()` read and write.
    pub(crate) fn thread_interrupt_flag(&self, id: i64) -> Option<Arc<AtomicBool>> {
        lock(&self.threads)
            .get(id as usize)
            .map(|slot| Arc::clone(&slot.interrupt))
    }

    /// Records the OS thread `start()` just spawned into an existing slot.
    pub(crate) fn start_thread(&self, id: i64, handle: JoinHandle<()>) {
        if let Some(slot) = lock(&self.threads).get_mut(id as usize) {
            slot.os_thread = Some(handle.thread().clone());
            slot.handle = Some(handle);
            slot.started = true;
        }
    }

    pub(crate) fn thread_is_started(&self, id: i64) -> bool {
        lock(&self.threads)
            .get(id as usize)
            .is_some_and(|slot| slot.started)
    }

    /// Requests interruption of the thread in slot `id`: sets its flag, then
    /// `unpark()`s it so a wait already blocked in `park_timeout` returns
    /// immediately instead of sitting out its poll interval. The flag is
    /// sticky — interrupting a thread that isn't blocked (or hasn't started)
    /// makes its *next* interruptible wait throw, rather than being dropped.
    pub(crate) fn interrupt_thread(&self, id: i64) {
        let threads = lock(&self.threads);
        let Some(slot) = threads.get(id as usize) else {
            return;
        };
        slot.interrupt.store(true, Ordering::SeqCst);
        if let Some(os_thread) = &slot.os_thread {
            os_thread.unpark();
        }
    }

    pub(crate) fn thread_is_interrupted(&self, id: i64) -> bool {
        lock(&self.threads)
            .get(id as usize)
            .is_some_and(|slot| slot.interrupt.load(Ordering::SeqCst))
    }

    /// True when no `system.thread.Thread` this program started is still
    /// executing bytecode — i.e. the calling thread is the only one that
    /// can currently be mutating the object graph. The cycle collector
    /// (`crate::gc`) refuses to run a pass unless this holds: trial
    /// deletion reads strong counts and fields as a series of snapshots,
    /// and a concurrent mutator could publish a new reference to a node
    /// *after* that node's count was read, making a live object look
    /// collectible. An empty/all-joined table (the overwhelmingly common
    /// case — a program that never spawns a thread) makes this a lock plus
    /// a walk over nothing.
    ///
    /// Deliberately also false when called *from* a spawned thread, which
    /// necessarily sees its own unfinished handle: only the main thread
    /// ever runs a pass, and only while every spawned thread has finished.
    pub(crate) fn no_vm_threads_running(&self) -> bool {
        lock(&self.threads)
            .iter()
            .all(|slot| slot.handle.as_ref().is_none_or(JoinHandle::is_finished))
    }

    /// True for a thread that ran to completion, and also for one that was
    /// never started or was already joined — every caller uses it as "not
    /// currently executing bytecode".
    pub(crate) fn thread_is_finished(&self, id: i64) -> bool {
        match lock(&self.threads).get(id as usize) {
            Some(slot) => slot.handle.as_ref().is_none_or(JoinHandle::is_finished),
            None => true,
        }
    }

    /// Takes the handle out (idempotent: a slot left empty by a previous
    /// `join()` reads back as "already finished", `Ok(())`). A genuine Rust
    /// panic inside the task (a VM bug, not an NL-level exception — those
    /// are caught and reported to stderr *inside* the task, see
    /// `crate::native::dispatch_thread`) is swallowed here rather than
    /// re-panicking the joining thread, matching vm.md's destructor
    /// contract stance that one component's failure shouldn't cascade.
    pub(crate) fn join_thread(&self, id: i64) {
        let handle = lock(&self.threads)
            .get_mut(id as usize)
            .and_then(|slot| slot.handle.take());
        if let Some(handle) = handle {
            let _ = handle.join();
        }
    }

    pub(crate) fn register_mutex(&self) -> i64 {
        let mut mutexes = lock(&self.thread_mutexes);
        mutexes.push(Some(Counter::new(1)));
        (mutexes.len() - 1) as i64
    }

    pub(crate) fn mutex(&self, id: i64) -> Option<Arc<Counter>> {
        lock(&self.thread_mutexes)
            .get(id as usize)
            .and_then(Clone::clone)
    }

    pub(crate) fn register_semaphore(&self, initial: i64) -> i64 {
        let mut semaphores = lock(&self.thread_semaphores);
        semaphores.push(Some(Counter::new(initial)));
        (semaphores.len() - 1) as i64
    }

    pub(crate) fn semaphore(&self, id: i64) -> Option<Arc<Counter>> {
        lock(&self.thread_semaphores)
            .get(id as usize)
            .and_then(Clone::clone)
    }
}

/// vm.md § Method dispatch: "each class has a method table (vtable)
/// computed at link time" — one flattened table per class, holding the
/// class's own methods *and* every method it inherits, so dispatch is a
/// lookup instead of a walk up `extends` recomputing the same answer at
/// every call site (nlvm issue #12).
///
/// Built by `verify_link`, which is already the one pass that has every
/// module in view at once, and handed to `Program::new`. Slots address
/// their method positionally (`module_index` into the same `Vec<Module>`
/// the `Program` is built from, then `method_index` into that module's
/// `methods`), so a resolved call costs two hashes (class, then method
/// name) and no string allocation at all — the pre-vtable walk allocated
/// one `String` per hop just to key the next lookup.
#[derive(Debug, Default)]
pub struct VTables {
    classes: HashMap<String, ClassVTable>,
}

/// Method name → the slots declared under it, ordered
/// nearest-declaration-first: a class's own methods (in declaration order)
/// before its superclass's, and so on up. Overloads legitimately share a
/// name, hence a `Vec` per name rather than one slot; the descriptor
/// discriminates them, exactly like the linear scan it replaces.
type ClassVTable = HashMap<String, Vec<VTableEntry>>;

#[derive(Debug)]
struct VTableEntry {
    descriptor: String,
    /// `descriptor[..params_len]` is the parameter portion, i.e. everything
    /// before the `" -> "` — precomputed here so a covariant lookup
    /// (`Program::resolve_method_covariant`) doesn't re-split the string on
    /// every candidate at every call.
    params_len: usize,
    module_index: usize,
    method_index: usize,
}

impl VTableEntry {
    fn params(&self) -> &str {
        &self.descriptor[..self.params_len]
    }
}

/// One flattened table per class. Ordering makes "nearest declaration
/// wins" — the rule the chain walk enforced by *stopping* at the first
/// match — hold for all three lookup shapes at once (full descriptor,
/// parameters only, name only): a subclass's slots are simply pushed
/// before its ancestors'. A slot whose name+descriptor a nearer class
/// already occupies is dropped rather than pushed: it is an override, and
/// the two share one vtable slot (the sense in which vm.md § Class flag
/// bits speaks of "a vtable slot occupied by" a `FINAL` method).
fn build_vtables(
    modules: &[Module],
    by_name: &HashMap<&str, usize>,
) -> Result<HashMap<String, ClassVTable>, VmError> {
    let mut classes = HashMap::with_capacity(by_name.len());
    for (&name, &index) in by_name {
        classes.insert(
            name.to_string(),
            build_class_vtable(modules, by_name, index)?,
        );
    }
    Ok(classes)
}

fn build_class_vtable(
    modules: &[Module],
    by_name: &HashMap<&str, usize>,
    start: usize,
) -> Result<ClassVTable, VmError> {
    let mut table = ClassVTable::new();
    let mut current = Some(start);
    let mut hops = 0usize;
    while let Some(index) = current {
        // A hierarchy that loops back on itself would make the walk below
        // (and every chain walk elsewhere in the VM) run forever; more
        // classes visited than the program contains means it does. Rejected
        // rather than truncated: a class whose ancestry can't be enumerated
        // is one whose vtable can't be claimed to be complete.
        if hops > by_name.len() {
            let name = modules[start].this_class_name().unwrap_or("?");
            return Err(VmError::Link(format!(
                "cyclic class hierarchy above class '{name}'"
            )));
        }
        hops += 1;

        let module = &modules[index];
        for (method_index, method) in module.methods.iter().enumerate() {
            let (Some(name), Some(descriptor)) = (
                module.constant_pool.utf8_at(method.name_index),
                module.constant_pool.type_desc_at(method.descriptor_index),
            ) else {
                continue;
            };
            let slots = table.entry(name.to_string()).or_default();
            if slots.iter().any(|slot| slot.descriptor == descriptor) {
                continue;
            }
            slots.push(VTableEntry {
                params_len: descriptor.find(" -> ").unwrap_or(descriptor.len()),
                descriptor: descriptor.to_string(),
                module_index: index,
                method_index,
            });
        }

        current = if module.super_class == 0 {
            None
        } else {
            // An unknown super class ends the walk instead of failing: the
            // native classes (`system.List`, ...) have no module, and the
            // walk this replaces stopped there too.
            module
                .constant_pool
                .class_name_at(module.super_class)
                .and_then(|n| by_name.get(n).copied())
        };
    }
    Ok(table)
}

/// vm.md § Class flag bits / § Method descriptor — the whole-program checks
/// the spec phrases as happening "at link time": a `super_class` naming a
/// `FINAL` class is rejected outright, a method that redeclares the same
/// name+descriptor as an ancestor's `FINAL` method is rejected as an illegal
/// override, and a `NEW` targeting an `ABSTRACT` class is rejected wherever
/// it appears in a code array (`verify_new_targets`). All three need every
/// module of the program to be loaded at once (a single `Module` only knows
/// its own `super_class`/`NEW` operand as a constant-pool *index*, not
/// whether the class it names carries a given flag), unlike
/// `nl_bytecode::Module::validate`'s single-module invariants (also run
/// here, once per module, so a program built in memory by `nl-codegen` —
/// see `nl-test-runner`, which never round-trips through `encode`/`decode`
/// — gets the same enforcement `Module::decode` already gives a `.nlm`
/// loaded from disk).
///
/// Also the point where each class's `VTables` entry is computed, for the
/// same reason: it is the one pass holding every module at once. The
/// returned tables index into `modules` positionally and belong to the
/// `Program` built from that exact vector.
pub fn verify_link(modules: &[Module]) -> Result<VTables, VmError> {
    let by_name: HashMap<&str, usize> = modules
        .iter()
        .enumerate()
        .filter_map(|(i, m)| m.this_class_name().map(|name| (name, i)))
        .collect();

    // First, because it is what rejects a hierarchy that loops back on
    // itself — the `extends` walks below (and everywhere else in the VM)
    // assume an acyclic one and would otherwise spin forever.
    let classes = build_vtables(modules, &by_name)?;

    for module in modules {
        module.validate()?;

        let Some(name) = module.this_class_name() else {
            continue;
        };

        if module.super_class != 0 {
            let super_name = module
                .constant_pool
                .class_name_at(module.super_class)
                .ok_or(VmError::Malformed("bad super_class index"))?;
            if by_name
                .get(super_name)
                .is_some_and(|&s| modules[s].class_flags & class_flags::FINAL != 0)
            {
                return Err(VmError::Link(format!(
                    "class '{name}' cannot extend final class '{super_name}'"
                )));
            }
        }

        // For each of this module's own methods, walk up the `extends`
        // chain looking for the nearest ancestor declaring the same
        // name+descriptor — the same "nearest wins" resolution virtual
        // dispatch itself uses (`resolve_virtual`/`find_method_by_
        // descriptor`). If that nearest declaration is `FINAL`, this
        // method illegally overrides it; if it isn't, further ancestors
        // don't matter (they're already shadowed by the nearer one, so
        // they don't own the vtable slot this method occupies).
        for m in &module.methods {
            if m.flags & (method_flags::CONSTRUCTOR | method_flags::DESTRUCTOR) != 0 {
                continue;
            }
            let (Some(method_name), Some(descriptor)) = (
                module.constant_pool.utf8_at(m.name_index),
                module.constant_pool.type_desc_at(m.descriptor_index),
            ) else {
                continue;
            };

            let mut ancestor = module
                .constant_pool
                .class_name_at(module.super_class)
                .and_then(|n| by_name.get(n).copied())
                .map(|i| &modules[i]);
            while let Some(anc) = ancestor {
                if let Some(anc_method) = anc.find_method_by_descriptor(method_name, descriptor) {
                    if anc_method.flags & method_flags::FINAL != 0 {
                        let anc_name = anc.this_class_name().unwrap_or("?");
                        return Err(VmError::Link(format!(
                            "method '{method_name}' in class '{name}' overrides final method declared in '{anc_name}'"
                        )));
                    }
                    break;
                }
                ancestor = anc
                    .constant_pool
                    .class_name_at(anc.super_class)
                    .and_then(|n| by_name.get(n).copied())
                    .map(|i| &modules[i]);
            }
        }

        verify_new_targets(module, name, modules, &by_name)?;
    }

    Ok(VTables { classes })
}

/// vm.md § Class flag bits, `ABSTRACT`: "The VM must reject `NEW` targeting
/// a class with this flag". The rejection the spec asks for is *static* —
/// it doesn't depend on the `NEW` being reached — so this sweeps every one
/// of `module`'s code arrays and resolves each `NEW`'s class operand,
/// catching the ones sitting in a branch that never executes. (`Opcode::New`
/// re-checks the same flag at runtime; that check is now a backstop for
/// programs that reached the interpreter without going through
/// `verify_link` at all, rather than the only enforcement point.)
///
/// The sweep is a plain linear decode (`nl_bytecode::disasm`) — sound here
/// because every instruction in this encoding is fixed-width with no inline
/// data, so "no `NEW` of an abstract class was found" really does mean
/// there is none. Code that doesn't decode cleanly (unknown opcode byte,
/// operands running off the end) is rejected rather than skipped: a method
/// whose instruction boundaries can't be recovered is one whose `NEW`s
/// can't be enumerated, so passing it would be an unverified claim.
///
/// A `NEW` naming a class the program doesn't contain is *not* an error
/// here: that's how the native no-backing-`Module` classes appear
/// (`system.List`, `system.Random`, `system.thread.Thread`, ... — see
/// `crate::native`), and `Opcode::New` handles them explicitly.
fn verify_new_targets(
    module: &Module,
    name: &str,
    modules: &[Module],
    by_name: &HashMap<&str, usize>,
) -> Result<(), VmError> {
    for method in &module.methods {
        for instruction in nl_bytecode::instructions(&method.code) {
            let instruction = instruction?;
            if instruction.opcode != nl_bytecode::Opcode::New {
                continue;
            }
            let target = instruction
                .operand_u16(0)
                .and_then(|idx| module.constant_pool.class_name_at(idx))
                .ok_or(VmError::Malformed("bad class index on NEW"))?;
            if by_name
                .get(target)
                .is_some_and(|&t| modules[t].class_flags & class_flags::ABSTRACT != 0)
            {
                let method_name = module
                    .constant_pool
                    .utf8_at(method.name_index)
                    .unwrap_or("?");
                return Err(VmError::Link(format!(
                    "method '{method_name}' in class '{name}' cannot instantiate abstract class '{target}'"
                )));
            }
        }
    }
    Ok(())
}

pub struct RunOutcome {
    pub exit_code: i32,
    /// Everything written via `system.Out.print`/`println` (see `crate::native`).
    pub stdout: String,
    /// Everything written via `system.Err.print`/`println`, plus the
    /// unhandled-exception message if any (see § Program startup, step 7).
    pub stderr: String,
}

/// Program startup — see nlvm-specs/docs/vm.md § Program startup.
///
/// Step 7 ("when main returns, ... exit") is taken literally: any
/// `system.thread.Thread` still running when `main` returns is abandoned,
/// not waited for (there is no "non-daemon thread" concept in the spec).
/// A conformant NL program that wants to wait for its worker threads calls
/// `join()` itself before returning from `main`, as every home-grown test
/// in this phase does.
pub fn run_program(modules: &[Module], program_args: &[String]) -> RunOutcome {
    run_program_impl(modules, program_args, None)
}

/// Same as `run_program`, but `system.In.readLine` reads from `stdin_data`
/// instead of the real process stdin — lets a caller (`nl-test-runner`'s
/// `Header::stdin`, see nlvm issue #6) script scanner input without a real
/// pipe.
pub fn run_program_with_stdin(
    modules: &[Module],
    program_args: &[String],
    stdin_data: &str,
) -> RunOutcome {
    run_program_impl(modules, program_args, Some(stdin_data.as_bytes().to_vec()))
}

fn run_program_impl(
    modules: &[Module],
    program_args: &[String],
    stdin_data: Option<Vec<u8>>,
) -> RunOutcome {
    // vm.md § Class flag bits / § Method descriptor — whole-program
    // structural checks, run once before anything (not even `<clinit>`)
    // executes, exactly like the "link time" wording in the spec implies.
    let vtables = match verify_link(modules) {
        Ok(vtables) => vtables,
        Err(e) => {
            return RunOutcome {
                exit_code: 1,
                stdout: String::new(),
                stderr: format!("{e}"),
            }
        }
    };

    let program = Arc::new(Program::new(modules.to_vec(), vtables, stdin_data));

    // vm.md § Program startup happens after every class's `static` storage
    // is in place — see `run_static_initializers`'s doc comment for why
    // this runs once, up front, rather than lazily per class on first use.
    // A `<clinit>` failure (an uncaught exception inside a static field
    // initializer) is reported exactly like an uncaught exception from
    // `main` itself; nothing has run yet, so there's no partial output to
    // preserve beyond whatever the failing initializer itself wrote.
    if let Err(e) = run_static_initializers(&program) {
        let (exit_code, error_line) = outcome_for_error(e);
        let stdout = lock(&program.stdout).clone();
        let mut stderr = lock(&program.stderr).clone();
        if let Some(line) = error_line {
            append_line(&mut stderr, &line);
        }
        return RunOutcome {
            exit_code,
            stdout,
            stderr,
        };
    }

    let Some((main_module, main)) = program.find_main() else {
        return RunOutcome {
            exit_code: 1,
            stdout: String::new(),
            stderr: format!("{}", VmError::NoMain),
        };
    };

    let args_array = Value::Array(Arc::new(Mutex::new(
        program_args
            .iter()
            .map(|s| Value::Str(Arc::new(s.clone())))
            .collect(),
    )));

    let result = call_static(&program, main_module, main, vec![args_array]);
    // The `result` value is fully consumed (and thus dropped) *before*
    // stdout/stderr are captured: an unhandled exception object may itself
    // have a `<destruct>` (see `Object`'s `Drop` impl) whose output must
    // land in the captured streams like any other destructor's.
    let (exit_code, error_line) = match result {
        Ok(Some(Value::Int(code))) => (code as i32, None),
        Ok(_) => (0, None),
        Err(e) => outcome_for_error(e),
    };
    // Same reasoning, for reference cycles (crate::gc): a cycle whose last
    // root disappeared without hitting an instrumented mutation site (see
    // `crate::gc`'s module doc) would otherwise sit uncollected — and its
    // destructor's output un-captured — until the process exits.
    crate::gc::final_sweep(&program);
    let stdout = lock(&program.stdout).clone();
    let mut stderr = lock(&program.stderr).clone();
    if let Some(line) = error_line {
        append_line(&mut stderr, &line);
    }
    RunOutcome {
        exit_code,
        stdout,
        stderr,
    }
}

/// Runs every loaded class's `<clinit>` (see `nl_codegen`'s
/// `compile_file`), in load order — a fixed, deterministic sequence rather
/// than Java-style lazy-on-first-use initialization, a documented
/// simplification like this codebase's other approximations (e.g.
/// reference-counting GC with a cycle collector rather than a tracing one).
/// A class with no static field carrying a declared initializer has no
/// `<clinit>` at all (`nl_codegen` only emits one when needed), so this is
/// a no-op for the overwhelming majority of classes.
fn run_static_initializers(program: &Arc<Program>) -> Result<(), VmError> {
    for module in &program.modules {
        // A module the constant pool doesn't name is one nothing can refer
        // to (`Program::new` gives it no `by_name` entry either) — skipped
        // rather than initialized.
        if module.this_class_name().is_none() {
            continue;
        }
        if let Some(clinit) = module.find_method("<clinit>") {
            call_static(program, module, clinit, Vec::new())?;
        }
    }
    Ok(())
}

/// Shared by `run_static_initializers`'s and `main`'s failure paths —
/// vm.md § Throw and stack unwinding, step 5.
fn outcome_for_error(e: VmError) -> (i32, Option<String>) {
    match e {
        VmError::Thrown(exc) => {
            let line = format!("Unhandled exception: {}", describe_exception(&exc));
            drop(exc);
            (1, Some(line))
        }
        // `system.ps.Process.exit(code)` — see `VmError::Exit`'s doc
        // comment. Not an error at all from the caller's point of view,
        // just an early, uncatchable short-circuit.
        VmError::Exit(code) => (code, None),
        e => (1, Some(format!("Unhandled exception: {e}"))),
    }
}

fn append_line(buf: &mut String, line: &str) {
    if !buf.is_empty() && !buf.ends_with('\n') {
        buf.push('\n');
    }
    buf.push_str(line);
}

/// `vm.md § Throw and stack unwinding`, step 5: "the VM prints the
/// exception message and stack trace to stderr". First line renders as
/// `ClassName: message` (or bare `ClassName` if `message` is absent/not a
/// string) — matches the implicit-exception wording already used by e.g.
/// `IndexOutOfBoundsException`. Followed by one `\tat file:line` per
/// `Exception.stackTrace` entry, if any (vm.md leaves the exact rendering
/// "implementation-defined" — no canonical format is specified).
pub(crate) fn describe_exception(exc: &Value) -> String {
    let Value::Object(obj) = exc else {
        return exc.to_display_string();
    };
    let obj = lock(obj);
    let header = match obj.fields.get("message") {
        Some(Value::Str(s)) if !s.is_empty() => format!("{}: {s}", obj.class_name),
        _ => obj.class_name.clone(),
    };
    let Some(Value::Array(frames)) = obj.fields.get("stackTrace") else {
        return header;
    };
    let frames = lock(frames);
    let mut out = header;
    for frame in frames.iter() {
        let Value::Object(point) = frame else {
            continue;
        };
        let point = lock(point);
        let file = match point.fields.get("file") {
            Some(Value::Str(s)) => s.as_str(),
            _ => "?",
        };
        let line = match point.fields.get("line") {
            Some(Value::Int(n)) => *n,
            _ => 0,
        };
        out.push_str(&format!("\n\tat {file}:{line}"));
    }
    out
}
