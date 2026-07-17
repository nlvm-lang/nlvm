//! vm.md § SET_FIELD: "the VM must reject writes to readonly fields outside
//! constructors at runtime (as a safety net; the compiler should have caught
//! this)". `nl-sema` already rejects this at compile time (E013/E014,
//! `phase7_0140`/`phase7_0150` in `tests/`), so exercising the VM's own
//! check requires skipping `nl_sema::check_compile` and going straight from
//! parser to `nl_codegen::compile_program` — simulating the "static check
//! got bypassed" scenario the safety net exists for.

fn compile(sources: &[&str]) -> Vec<nl_bytecode::Module> {
    let files: Vec<_> = sources
        .iter()
        .map(|src| nl_syntax::parse_source_file(src).expect("parse"))
        .collect();
    nl_codegen::compile_program(&files).expect("codegen")
}

#[test]
fn set_field_rejects_readonly_write_outside_constructor() {
    let money = r#"
namespace phase7.e013.runtime;
class readonly Money {
	public int cents;

	public construct(int cents) {
		this.cents = cents;
	}
}
"#;
    let main = r#"
namespace phase7.e013.runtime;
class Main {
	public static int main(string[] args) {
		auto m = new Money(100);
		m.cents = 200;
		return 0;
	}
}
"#;
    let modules = compile(&[money, main]);
    let outcome = nl_vm::run_program(&modules, &[]);
    assert_eq!(
        outcome.exit_code, 1,
        "stdout={:?} stderr={:?}",
        outcome.stdout, outcome.stderr
    );
    assert!(
        outcome.stderr.contains("readonly"),
        "stderr={:?}",
        outcome.stderr
    );
}

#[test]
fn set_field_allows_readonly_write_inside_declaring_constructor() {
    let money = r#"
namespace phase7.e013.runtime_ok;
class readonly Money {
	public int cents;

	public construct(int cents) {
		this.cents = cents;
	}
}
"#;
    let main = r#"
namespace phase7.e013.runtime_ok;
class Main {
	public static int main(string[] args) {
		auto m = new Money(100);
		return m.cents;
	}
}
"#;
    let modules = compile(&[money, main]);
    let outcome = nl_vm::run_program(&modules, &[]);
    assert_eq!(
        outcome.exit_code, 100,
        "stdout={:?} stderr={:?}",
        outcome.stdout, outcome.stderr
    );
}
