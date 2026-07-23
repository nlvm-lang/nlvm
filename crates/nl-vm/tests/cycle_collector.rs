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
