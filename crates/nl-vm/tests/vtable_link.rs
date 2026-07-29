//! vm.md § Method dispatch — the link-time vtable build (nlvm issue #12).
//! The dispatch results themselves are covered end to end by `tests/
//! phase24_0010`; what needs a Rust-level test is the malformed hierarchy
//! `nl-sema` would normally have rejected long before (E037, cyclic
//! `extends`), reached here the same way `abstract_final_link.rs` reaches
//! its cases: parser straight to `nl_codegen::compile_program`, skipping
//! `nl_sema::check_compile`.

fn compile(sources: &[&str]) -> Vec<nl_bytecode::Module> {
    let files: Vec<_> = sources
        .iter()
        .map(|src| nl_syntax::parse_source_file(src, "test").expect("parse"))
        .collect();
    nl_codegen::compile_program(&files).expect("codegen")
}

/// A hierarchy that loops back on itself has no well-defined vtable, and
/// every `extends` walk in the VM (dispatch before this change, the
/// final-override check in `verify_link`, `is_instance_of`) would spin on
/// it forever. Linking must reject it instead of hanging.
#[test]
fn verify_link_rejects_a_cyclic_hierarchy() {
    let a = r#"
namespace test.vtable.cycle;
class A extends B {
	public construct() {}
	public string label() {
		return "a";
	}
}
"#;
    let b = r#"
namespace test.vtable.cycle;
class B extends A {
	public construct() {}
}
"#;
    let modules = compile(&[a, b]);

    let err = nl_vm::verify_link(&modules).expect_err("a cyclic hierarchy must be rejected");
    assert!(matches!(err, nl_vm::VmError::Link(_)));
    assert!(format!("{err}").contains("cyclic"), "err={err}");

    let outcome = nl_vm::run_program(&modules, &[]);
    assert_eq!(outcome.exit_code, 1);
    assert!(
        outcome.stderr.contains("cyclic"),
        "stderr={:?}",
        outcome.stderr
    );
}

/// A superclass the program doesn't contain ends the walk instead of
/// failing it: that is how a class extending a native one (no backing
/// `Module` — see `nl_vm::native`) appears at link time, and the chain
/// walk this vtable replaced stopped there too.
#[test]
fn verify_link_accepts_a_superclass_with_no_module() {
    let sub = r#"
namespace test.vtable.nomodule;
class Sub extends Nonexistent {
	public construct() {}
	public string label() {
		return "sub";
	}
}
"#;
    let modules = compile(&[sub]);
    nl_vm::verify_link(&modules).expect("an unknown superclass is not a link error");
}
