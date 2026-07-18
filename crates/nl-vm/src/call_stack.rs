//! vm.md § Stack trace construction — a thread-local shadow stack of active
//! NL call frames, maintained in parallel to the interpreter's native Rust
//! recursion. `interpreter::run_frame` has no explicit `Frame`/`CallStack`
//! struct of its own (a method call is just a recursive Rust call through
//! `call_static`/`call_instance`), so there is nothing else to walk when an
//! `Exception` needs to capture where it was constructed. Real OS threads
//! (`native::construct_thread`) each get their own independent stack for
//! free, since `thread_local!` storage is per-OS-thread.

use std::cell::{Cell, RefCell};

use nl_bytecode::LineTableEntry;

/// One active NL call frame — enough to resolve an `ExecutionPoint` (line +
/// declaring class) at any point during this frame's execution.
struct FrameInfo {
    class_fqcn: String,
    method_name: String,
    line: Cell<u32>,
}

thread_local! {
    static STACK: RefCell<Vec<FrameInfo>> = const { RefCell::new(Vec::new()) };
}

/// vm.md § Call frame — "dépassement de profondeur → StackOverflowException"
/// (docs/02_done_stack_trace.md step 3). Each `run_frame` invocation is a real Rust
/// stack frame (method calls recurse natively, see module doc comment), so
/// this must stay well under what the host thread's native stack can hold,
/// with enough margin to survive an **unoptimized debug build** (several
/// times more stack per frame than release) on the smallest stack a program
/// might recurse on. `native::dispatch_thread`'s `system.thread.Thread`
/// spawn gives its OS thread an explicit 8 MiB stack (matching a typical
/// Linux main-thread default, `ulimit -s`) specifically so this one constant
/// stays safe on every thread a program can recurse on, not just the main
/// one.
///
/// 300 was the empirically observed crash threshold on this machine, in a
/// debug build, for both the main thread and an explicitly-8-MiB spawned
/// thread (measured by bisecting a `recurse(n) { return recurse(n-1)+1; }`
/// style program — deeper stack use per call than a tail-shaped recurse, so
/// a reasonable stand-in for "worst case" NL code). 150 keeps a ~2x margin
/// below that for slower/other platforms, while still allowing genuinely
/// deep recursion (tree walks, naive Fibonacci, etc.) to run to completion
/// instead of hitting the ceiling for everyday use.
const MAX_CALL_DEPTH: usize = 150;

/// RAII guard returned by `push_frame`: pops the frame when dropped, which
/// happens on every exit path out of `run_frame` (normal return, `?`
/// propagation, or an unhandled `VmError`) without needing a matching manual
/// pop at each of those sites.
pub struct FrameGuard {
    _private: (),
}

impl Drop for FrameGuard {
    fn drop(&mut self) {
        STACK.with(|s| {
            s.borrow_mut().pop();
        });
    }
}

/// Pushes a frame for a method about to start executing. Called once at the
/// top of `run_frame`, before its instruction loop. Fails (without pushing)
/// once this thread's shadow stack already holds `MAX_CALL_DEPTH` frames —
/// the caller must throw `StackOverflowException` instead of entering the
/// new frame.
pub fn push_frame(class_fqcn: String, method_name: String) -> Result<FrameGuard, ()> {
    STACK.with(|s| {
        let mut stack = s.borrow_mut();
        if stack.len() >= MAX_CALL_DEPTH {
            return Err(());
        }
        stack.push(FrameInfo {
            class_fqcn,
            method_name,
            line: Cell::new(0),
        });
        Ok(())
    })?;
    Ok(FrameGuard { _private: () })
}

/// Updates the topmost (current) frame's source line — called once per
/// instruction from `run_frame`'s loop, before executing it, so the frame
/// always reflects the line of whichever instruction is about to run.
pub fn set_current_line(line_table: &[LineTableEntry], pc: usize) {
    let line = line_for_pc(line_table, pc);
    STACK.with(|s| {
        if let Some(frame) = s.borrow().last() {
            frame.line.set(line);
        }
    });
}

/// vm.md § Method descriptor (line-number table): entries are sorted by
/// ascending `start_pc`, each covering offsets up to the next entry's
/// `start_pc`. Yields `0` if the table is absent (stripped build, or a
/// closure with an expression body — see `nl_codegen`'s `record_line`) or
/// `pc` precedes every entry.
fn line_for_pc(line_table: &[LineTableEntry], pc: usize) -> u32 {
    let pc = pc as u16;
    let idx = line_table.partition_point(|e| e.start_pc <= pc);
    idx.checked_sub(1).map(|i| line_table[i].line).unwrap_or(0)
}

/// Snapshots every currently active frame on this thread, innermost
/// (current) first — vm.md § Stack trace construction: "the VM natively
/// walks the current call stack". `skip` drops that many innermost frames;
/// `interpreter::maybe_capture_stack_trace`/`throw_native` use this to
/// exclude the exception hierarchy's own constructor chain so the trace
/// starts at the `new` site (or, for a VM-thrown exception, at the fault
/// site itself — `skip = 0`, no constructor chain involved).
pub fn snapshot(skip: usize) -> Vec<(String, String, u32)> {
    STACK.with(|s| {
        s.borrow()
            .iter()
            .rev()
            .skip(skip)
            .map(|f| (f.class_fqcn.clone(), f.method_name.clone(), f.line.get()))
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_for_pc_picks_covering_entry() {
        let table = vec![
            LineTableEntry {
                start_pc: 0,
                line: 3,
            },
            LineTableEntry {
                start_pc: 5,
                line: 4,
            },
            LineTableEntry {
                start_pc: 12,
                line: 6,
            },
        ];
        assert_eq!(line_for_pc(&table, 0), 3);
        assert_eq!(line_for_pc(&table, 4), 3);
        assert_eq!(line_for_pc(&table, 5), 4);
        assert_eq!(line_for_pc(&table, 11), 4);
        assert_eq!(line_for_pc(&table, 12), 6);
        assert_eq!(line_for_pc(&table, 999), 6);
    }

    #[test]
    fn line_for_pc_empty_table_is_zero() {
        assert_eq!(line_for_pc(&[], 0), 0);
        assert_eq!(line_for_pc(&[], 42), 0);
    }

    #[test]
    fn push_frame_tracks_line_and_pops_on_drop() {
        // Isolated by thread_local — safe to run alongside other tests.
        assert_eq!(snapshot(0), Vec::<(String, String, u32)>::new());
        {
            let _f1 = push_frame("Ns.A".to_string(), "main".to_string()).unwrap();
            let table = vec![LineTableEntry {
                start_pc: 0,
                line: 10,
            }];
            set_current_line(&table, 0);
            {
                let _f2 = push_frame("Ns.B".to_string(), "helper".to_string()).unwrap();
                set_current_line(&table, 0);
                assert_eq!(
                    snapshot(0),
                    vec![
                        ("Ns.B".to_string(), "helper".to_string(), 10),
                        ("Ns.A".to_string(), "main".to_string(), 10),
                    ]
                );
                assert_eq!(
                    snapshot(1),
                    vec![("Ns.A".to_string(), "main".to_string(), 10)]
                );
            }
            assert_eq!(
                snapshot(0),
                vec![("Ns.A".to_string(), "main".to_string(), 10)]
            );
        }
        assert_eq!(snapshot(0), Vec::<(String, String, u32)>::new());
    }

    #[test]
    fn push_frame_rejects_past_max_depth() {
        assert_eq!(snapshot(0), Vec::<(String, String, u32)>::new());
        let mut guards = Vec::with_capacity(MAX_CALL_DEPTH);
        for _ in 0..MAX_CALL_DEPTH {
            guards.push(push_frame("Ns.A".to_string(), "recurse".to_string()).unwrap());
        }
        assert!(push_frame("Ns.A".to_string(), "recurse".to_string()).is_err());
        assert_eq!(snapshot(0).len(), MAX_CALL_DEPTH);
        drop(guards);
        assert_eq!(snapshot(0), Vec::<(String, String, u32)>::new());
    }
}
