//! `crate::gc` — the trial-deletion cycle collector that fills the gap
//! `value.rs`'s module doc calls out ("objects in a reference cycle are
//! never reclaimed" was true before this collector existed). The YAML
//! fixtures in `tests/` (`phase15_00*`) cover the everyday shapes (two
//! objects, self-reference, array-mediated, rescue-by-root); this file
//! covers scenarios that are awkward to express as an exact `stdout`
//! comparison — a cycle whose member count/processing order isn't fixed,
//! a stress loop, and the "destructor called at most once" guarantee
//! specifically for a *cycle*-collected object (not just an ordinary
//! `Arc`-refcounted one, already covered by `phase7_0280` for the acyclic
//! case).

fn compile(sources: &[&str]) -> Vec<nl_bytecode::Module> {
    let files: Vec<_> = sources
        .iter()
        .map(|src| nl_syntax::parse_source_file(src, "test").expect("parse"))
        .collect();
    nl_codegen::compile_program(&files).expect("codegen")
}

/// A ring of 3 objects (a→b→c→a) — not just a 2-object mutual cycle — all
/// going out of scope together. Processing order across a >2-node cycle is
/// unspecified (this collector's internal bookkeeping is a `HashMap`), so
/// this asserts only the *count* of destructors run, not which ran first.
#[test]
fn three_node_ring_cycle_is_fully_collected() {
    let node = r#"
namespace test.cycle.ring3;
class Node {
	public static int destroyedCount = 0;
	public Node|null next;
	public construct() {}
	public destruct() {
		Node.destroyedCount = Node.destroyedCount + 1;
	}
}
"#;
    let main = r#"
namespace test.cycle.ring3;
class Main {
	public static void makeRing() {
		Node a = new Node();
		Node b = new Node();
		Node c = new Node();
		a.next = b;
		b.next = c;
		c.next = a;
	}

	public static int main(string[] args) {
		Main.makeRing();
		return Node.destroyedCount;
	}
}
"#;
    let modules = compile(&[node, main]);
    let outcome = nl_vm::run_program(&modules, &[]);
    assert_eq!(
        outcome.exit_code, 3,
        "expected all 3 ring members destroyed, stdout={:?} stderr={:?}",
        outcome.stdout, outcome.stderr
    );
}

/// Many independent small cycles, formed and abandoned in a loop — a
/// regression/stress check that repeated collection passes stay correct
/// (no double counting, no missed cycles) rather than just working once.
#[test]
fn repeated_small_cycles_all_collected() {
    let node = r#"
namespace test.cycle.stress;
class Node {
	public static int destroyedCount = 0;
	public Node|null next;
	public construct() {}
	public destruct() {
		Node.destroyedCount = Node.destroyedCount + 1;
	}
}
"#;
    let main = r#"
namespace test.cycle.stress;
class Main {
	public static void makePair() {
		Node a = new Node();
		Node b = new Node();
		a.next = b;
		b.next = a;
	}

	public static int main(string[] args) {
		int i = 0;
		while (i < 50) {
			Main.makePair();
			i = i + 1;
		}
		return Node.destroyedCount;
	}
}
"#;
    let modules = compile(&[node, main]);
    let outcome = nl_vm::run_program(&modules, &[]);
    assert_eq!(
        outcome.exit_code, 100,
        "expected 50 pairs * 2 nodes = 100 destroyed, stdout={:?} stderr={:?}",
        outcome.stdout, outcome.stderr
    );
}

/// vm.md § Garbage collection contract: "A destructor is called at most
/// once per object" — already guaranteed for the ordinary (acyclic) case
/// by `Object`'s own `Drop` impl (`phase7_0280` in `tests/`). This checks
/// the same guarantee holds when the object was reclaimed by the cycle
/// collector (`crate::gc::force_destroy`) instead: the destructor escapes
/// its resurrection copy (`this`) into a `static` field — the same
/// "leaked back into a live structure" scenario `value.rs`'s `Object::drop`
/// doc comment describes — and this checks the escaped copy's own later
/// death does *not* trigger a second `destruct()` call.
#[test]
fn cycle_collected_destructor_resurrection_runs_at_most_once() {
    let node = r#"
namespace test.cycle.resurrect;
class Node {
	public static int destructCount = 0;
	public static Node|null escaped;
	public Node|null next;
	public construct() {}
	public destruct() {
		Node.destructCount = Node.destructCount + 1;
		Node.escaped = this;
	}
}
"#;
    let main = r#"
namespace test.cycle.resurrect;
class Main {
	public static void makeCycle() {
		Node a = new Node();
		Node b = new Node();
		a.next = b;
		b.next = a;
	}

	public static int main(string[] args) {
		Main.makeCycle();
		int afterCycle = Node.destructCount;
		Node.escaped = null;
		int afterClear = Node.destructCount;
		return afterCycle * 100 + afterClear;
	}
}
"#;
    let modules = compile(&[node, main]);
    let outcome = nl_vm::run_program(&modules, &[]);
    // 2 destructions from collecting the cycle, then 2 again after clearing
    // `escaped` (unchanged — proves neither resurrection copy was
    // destructed a second time).
    assert_eq!(
        outcome.exit_code, 202,
        "stdout={:?} stderr={:?}",
        outcome.stdout, outcome.stderr
    );
}

/// A cycle whose last root is on the operand stack of an expression that
/// an exception aborts half-way through: the operands are dropped by
/// `run_frame`'s unwind path (formerly a bare `stack.clear()`), not by any
/// named slot going away. Each one is now noted as a collector candidate
/// on the way out, like a `POP`; this checks the cycle is reclaimed while
/// the program is still running (the counter is read *before* returning,
/// so `final_sweep` can't be what did it) and, just as importantly, that
/// noting mid-unwind doesn't disturb the unwind itself.
#[test]
fn cycle_abandoned_by_exception_unwinding_is_collected() {
    let node = r#"
namespace test.cycle.unwind;
class Node {
	public static int destroyedCount = 0;
	public Node|null next;
	public construct() {}
	public destruct() {
		Node.destroyedCount = Node.destroyedCount + 1;
	}
}
"#;
    let main = r#"
namespace test.cycle.unwind;
class Main {
	public static Node makeCycle() {
		Node a = new Node();
		Node b = new Node();
		a.next = b;
		b.next = a;
		return a;
	}

	public static int boom(Node n) throws Exception {
		throw new Exception("abort");
	}

	public static int main(string[] args) {
		try {
			// `makeCycle()`'s result sits on the operand stack as an
			// argument of a call that never completes: the handler's
			// unwind is what drops it.
			int ignored = Main.boom(Main.makeCycle());
		} catch (Exception e) {
			// Any durable-slot event after the unwind drains the noted
			// candidates; the catch parameter's own store is one.
		}
		return Node.destroyedCount;
	}
}
"#;
    let modules = compile(&[node, main]);
    let outcome = nl_vm::run_program(&modules, &[]);
    assert_eq!(
        outcome.exit_code, 2,
        "expected the unwound cycle collected before main returned, stdout={:?} stderr={:?}",
        outcome.stdout, outcome.stderr
    );
}

/// The collector refuses to run a pass while a spawned `system.thread.
/// Thread` could be mutating the same object graph (`crate::gc` §
/// Threads), so a cycle abandoned while a worker runs stays pending —
/// and is collected by the first pass after `join()`. This checks the
/// gate defers collection rather than losing it: the count is read after
/// the join but still inside `main`, so `final_sweep` can't be what
/// reclaimed it.
#[test]
fn cycle_abandoned_during_a_thread_is_collected_once_it_joins() {
    let node = r#"
namespace test.cycle.threaded;
class Node {
	public static int destroyedCount = 0;
	public Node|null next;
	public construct() {}
	public destruct() {
		Node.destroyedCount = Node.destroyedCount + 1;
	}
}
"#;
    let main = r#"
namespace test.cycle.threaded;
class Main {
	public static void makeCycle() {
		Node a = new Node();
		Node b = new Node();
		a.next = b;
		b.next = a;
	}

	public static int main(string[] args) {
		system.thread.Thread worker = new system.thread.Thread(() => {
			system.thread.Thread.sleep(20);
		});
		worker.start();
		// Abandoned while the worker is alive: noted, but no pass runs.
		Main.makeCycle();
		worker.join();
		// First durable-slot event after the join — the collector is
		// unblocked and drains everything noted meanwhile.
		int done = 1;
		return Node.destroyedCount * done;
	}
}
"#;
    let modules = compile(&[node, main]);
    let outcome = nl_vm::run_program(&modules, &[]);
    assert_eq!(
        outcome.exit_code, 2,
        "expected the deferred cycle collected after join, stdout={:?} stderr={:?}",
        outcome.stdout, outcome.stderr
    );
}
