//! vm.md § Class flag bits, `ABSTRACT` — the *link-time* half of "the VM
//! must reject `NEW` targeting a class with this flag" (nlvm issue #16).
//! `Opcode::New`'s runtime check only fires on a `NEW` that actually
//! executes; `verify_link` now sweeps every code array up front, so a `NEW`
//! sitting in a never-called method is rejected too.
//!
//! Like `abstract_final_link.rs`, these go straight from parser to
//! `nl_codegen::compile_program`, skipping `nl_sema::check_compile` (whose
//! E032 already rejects this at compile time) — the VM-side guard only
//! exists for bytecode that reached the VM without that check.

use nl_bytecode::{instructions, Module, Opcode};

fn compile(sources: &[&str]) -> Vec<Module> {
    let files: Vec<_> = sources
        .iter()
        .map(|src| nl_syntax::parse_source_file(src, "test").expect("parse"))
        .collect();
    nl_codegen::compile_program(&files).expect("codegen")
}

const SHAPE: &str = r#"
namespace test.newverify;
abstract class Shape {
	public construct() {}
	public abstract float area();
}
"#;

const CIRCLE: &str = r#"
namespace test.newverify;
class Circle extends Shape {
	public construct() {}
	public float area() {
		return 1.0;
	}
}
"#;

#[test]
fn verify_link_rejects_new_of_abstract_class_in_unreached_code() {
    // `make()` is never called, so the runtime check in `Opcode::New` never
    // sees this `NEW` — before the static sweep, the program exited 0.
    let main = r#"
namespace test.newverify;
class Main {
	public static Shape make() {
		return new Shape();
	}
	public static int main(string[] args) {
		return 0;
	}
}
"#;
    let modules = compile(&[SHAPE, main]);

    let err = nl_vm::verify_link(&modules).expect_err("NEW of an abstract class must be rejected");
    assert!(matches!(err, nl_vm::VmError::Link(_)), "err={err:?}");
    let text = format!("{err}");
    assert!(text.contains("abstract"), "err={text}");
    assert!(text.contains("test.newverify.Shape"), "err={text}");
    assert!(text.contains("make"), "err={text}");

    // And the same failure is what `run_program` reports — nothing runs at
    // all, so `main`'s own output never appears.
    let outcome = nl_vm::run_program(&modules, &[]);
    assert_eq!(outcome.exit_code, 1);
    assert!(outcome.stderr.contains("abstract"), "{:?}", outcome.stderr);
}

#[test]
fn verify_link_rejects_new_of_abstract_class_in_a_dead_branch() {
    let main = r#"
namespace test.newverify;
class Main {
	public static int main(string[] args) {
		if (args.length() > 100) {
			auto s = new Shape();
		}
		return 0;
	}
}
"#;
    let modules = compile(&[SHAPE, main]);
    let err = nl_vm::verify_link(&modules).expect_err("NEW of an abstract class must be rejected");
    assert!(format!("{err}").contains("abstract"), "err={err:?}");
}

#[test]
fn verify_link_accepts_new_of_a_concrete_subclass() {
    let main = r#"
namespace test.newverify;
class Main {
	public static Shape make() {
		return new Circle();
	}
	public static int main(string[] args) {
		return 0;
	}
}
"#;
    let modules = compile(&[SHAPE, CIRCLE, main]);
    nl_vm::verify_link(&modules).expect("instantiating a concrete subclass is legal");
    assert_eq!(nl_vm::run_program(&modules, &[]).exit_code, 0);
}

/// The sweep must not choke on `NEW`s of the native classes that have no
/// backing `Module` at all (`system.List`, `system.Random`, ... — see
/// `nl_vm::native`): an unresolvable target is "not an abstract class in
/// this program", not an error.
#[test]
fn verify_link_accepts_new_of_native_classes_without_a_module() {
    let main = r#"
namespace test.newverify.native;
class Main {
	public static int main(string[] args) {
		auto items = new system.List<int>();
		auto rng = new system.Random(1);
		items.add(2);
		return 0;
	}
}
"#;
    let modules = compile(&[main]);
    nl_vm::verify_link(&modules).expect("native classes have no module and no ABSTRACT flag");
    assert_eq!(nl_vm::run_program(&modules, &[]).exit_code, 0);
}

/// `Opcode::operand_len` (the table the sweep walks with) has to agree with
/// what `nl_vm::interpreter::exec_step` actually consumes per opcode —
/// disagree by one byte anywhere and the sweep desynchronizes, reading
/// operand bytes as opcodes and possibly missing a `NEW` entirely.
///
/// Checked empirically against real generated code rather than by
/// restating the table: every method of a program exercising the bulk of
/// the instruction set must decode to *exactly* the end of its code array,
/// and every `start_pc`/`handler_pc` nl-codegen recorded in a line table or
/// exception table must land on a boundary the sweep also found (codegen
/// only ever records instruction starts).
#[test]
fn operand_len_matches_generated_code_boundaries() {
    let greeter = r#"
namespace test.newverify.widths;
interface Greeter {
	string greet();
}
"#;
    let counter = r#"
namespace test.newverify.widths;
class Counter implements Greeter {
	public static int total;
	private int n;
	public construct(int start) {
		this.n = start;
	}
	public string greet() {
		return "n=" + system.Int.toString(this.n);
	}
	public int bump(int by) {
		for (int i = 0; i < by; i++) {
			this.n = this.n + 1;
			Counter.total++;
		}
		return this.n;
	}
}
"#;
    let main = r#"
namespace test.newverify.widths;
class Main {
	public static int main(string[] args) {
		auto c = new Counter(3);
		int[] xs = new int[]{1, 2, 3};
		float f = 2.5;
		bool ok = xs.length() > 0 && f < 10.0;
		string s = c.greet();
		auto fn = (int x) => x * 2;
		try {
			if (!ok) {
				throw new Exception("nope");
			}
			s = s + system.Int.toString(fn(xs[0]));
			c.bump(2);
		} catch (Exception e) {
			s = e.message;
		}
		Greeter g = c;
		if (g instanceof Counter) {
			s = s + g.greet();
		}
		while (Counter.total > 100) {
			Counter.total = Counter.total - 1;
		}
		system.Out.println(s);
		return 0;
	}
}
"#;
    let modules = compile(&[greeter, counter, main]);
    // The prelude modules are compiled in too, which is deliberate: they
    // widen the instruction mix this walks well beyond the source above.
    let mut seen_opcodes = std::collections::HashSet::new();
    let mut methods_walked = 0usize;

    for module in &modules {
        let class_name = module.this_class_name().unwrap_or("?").to_string();
        for method in &module.methods {
            if method.code.is_empty() {
                continue; // ABSTRACT / interface method — nothing to decode
            }
            methods_walked += 1;
            let name = module
                .constant_pool
                .utf8_at(method.name_index)
                .unwrap_or("?");
            let mut boundaries = std::collections::HashSet::new();
            let mut end = 0usize;
            for instruction in instructions(&method.code) {
                let instruction = instruction
                    .unwrap_or_else(|e| panic!("{class_name}.{name} failed to decode: {e}"));
                seen_opcodes.insert(instruction.opcode);
                boundaries.insert(instruction.pc);
                end = instruction.pc + 1 + instruction.opcode.operand_len();
            }
            assert_eq!(
                end,
                method.code.len(),
                "{class_name}.{name}: sweep ended at {end} of {} bytes — an operand width is wrong",
                method.code.len()
            );
            for entry in &method.line_table {
                assert!(
                    boundaries.contains(&(entry.start_pc as usize)),
                    "{class_name}.{name}: line-table pc {} is not an instruction boundary",
                    entry.start_pc
                );
            }
            for entry in &method.exception_table {
                for pc in [entry.start_pc, entry.handler_pc] {
                    assert!(
                        boundaries.contains(&(pc as usize)),
                        "{class_name}.{name}: exception-table pc {pc} is not an instruction boundary"
                    );
                }
            }
        }
    }

    assert!(methods_walked > 10, "expected a broad corpus to walk");
    // Guard against the corpus quietly shrinking to something that no
    // longer exercises the wide-operand opcodes this test exists for.
    for op in [
        Opcode::New,
        Opcode::IInc,
        Opcode::NewArrayInit,
        Opcode::BiPush,
        Opcode::InvokeInstance,
        Opcode::GetField,
    ] {
        assert!(seen_opcodes.contains(&op), "corpus never emitted {op:?}");
    }
}
