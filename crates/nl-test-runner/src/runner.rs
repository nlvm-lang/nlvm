use nl_bytecode::{Module, OptLevel};
use nl_vm::RunOutcome;

use crate::header::Header;
use crate::testfile::TestFile;

pub enum Outcome {
    Pass,
    /// Differential mode only: the fixture passed at both levels, but the
    /// two runs were deliberately not compared with each other — see
    /// `Header::optimization_sensitive`.
    PassNotCompared,
    Fail(String),
}

/// How a fixture is exercised.
#[derive(Debug, Clone, Copy)]
pub enum Mode {
    /// Compile and run once, at this level.
    Single(OptLevel),
    /// optimizations.md § Testing — "Run the same program with and without
    /// optimizations; compare outputs, exit codes, and exception behavior".
    /// The fixture's own expectations are checked at *both* levels, and the
    /// two runs must additionally agree with each other, which catches an
    /// optimization that changes something the fixture doesn't pin down
    /// (stderr on a fixture that only asserts an exit code, say).
    Differential,
}

/// What running a fixture at one level produced. `NotRun` covers the
/// fixtures that assert only compile-time behaviour (expected parse error,
/// expected compile error, `compile_only`, no expected exit code) — nothing
/// executed, so there is nothing to compare between levels.
enum Executed {
    NotRun,
    Ran(RunOutcome),
}

pub fn run_test(test: &TestFile, mode: Mode) -> Outcome {
    match mode {
        Mode::Single(level) => match run_at(test, level) {
            Ok(_) => Outcome::Pass,
            Err(reason) => Outcome::Fail(reason),
        },
        Mode::Differential => run_differential(test),
    }
}

fn run_differential(test: &TestFile) -> Outcome {
    let unoptimized = match run_at(test, OptLevel::O0) {
        Ok(e) => e,
        Err(reason) => return Outcome::Fail(format!("at -O0: {reason}")),
    };
    let optimized = match run_at(test, OptLevel::O1) {
        Ok(e) => e,
        Err(reason) => return Outcome::Fail(format!("at -O1: {reason}")),
    };

    // optimizations.md § Testing: a fixture whose output carries a stack
    // trace, or whose termination depends on stack exhaustion, is allowed to
    // differ between levels and must be excluded from the comparison rather
    // than reported as a failure. It still had to satisfy its own
    // expectations at both levels above, which is the part that stays
    // meaningful.
    if test.header.is_optimization_sensitive() {
        return Outcome::PassNotCompared;
    }

    match (unoptimized, optimized) {
        (Executed::NotRun, Executed::NotRun) => Outcome::Pass,
        (Executed::Ran(a), Executed::Ran(b)) => match diff(&a, &b) {
            Some(reason) => Outcome::Fail(reason),
            None => Outcome::Pass,
        },
        // Only reachable if optimization changed whether the program runs
        // at all — a bug in the pipeline, not in the fixture.
        _ => Outcome::Fail(
            "the program ran at one optimization level but not at the other".to_string(),
        ),
    }
}

/// The observable behaviour optimizations.md § Observability requires to be
/// identical: output, exit code, and — since an unhandled exception's
/// message and stack trace land on stderr (vm.md § Program startup) —
/// exception behaviour.
fn diff(unoptimized: &RunOutcome, optimized: &RunOutcome) -> Option<String> {
    if unoptimized.exit_code != optimized.exit_code {
        return Some(format!(
            "exit code differs between optimization levels: -O0 gave {}, -O1 gave {}",
            unoptimized.exit_code, optimized.exit_code
        ));
    }
    if unoptimized.stdout != optimized.stdout {
        return Some(format!(
            "stdout differs between optimization levels: -O0 gave {:?}, -O1 gave {:?}",
            unoptimized.stdout, optimized.stdout
        ));
    }
    if unoptimized.stderr != optimized.stderr {
        return Some(format!(
            "stderr differs between optimization levels: -O0 gave {:?}, -O1 gave {:?}",
            unoptimized.stderr, optimized.stderr
        ));
    }
    None
}

/// Checks every expectation the fixture states, at one optimization level.
/// `Err` is the failure reason; `Ok` says whether a program actually ran and
/// carries its output for the differential comparison.
fn run_at(test: &TestFile, level: OptLevel) -> Result<Executed, String> {
    let mut files = Vec::new();
    for block in &test.blocks {
        match nl_syntax::parse_source_file(&block.content, block.path.clone()) {
            Ok(f) => files.push(f),
            Err(e) => {
                return if test.header.expected_parse_error == Some(true) {
                    Ok(Executed::NotRun)
                } else {
                    Err(format!("parse error in {}: {e}", block.path))
                };
            }
        }
    }
    if test.header.expected_parse_error == Some(true) {
        return Err("expected a parse error but parsing succeeded".to_string());
    }

    let warnings = match nl_sema::check_compile_with_warnings(&mut files) {
        Ok(warnings) => warnings,
        Err(e) => {
            return match &test.header.expected_compile_error {
                Some(code) if code == e.code() => Ok(Executed::NotRun),
                Some(code) => Err(format!(
                    "expected compile error {code}, got {} ({e})",
                    e.code()
                )),
                None => Err(format!("unexpected compile error: {e}")),
            };
        }
    };
    if let Some(code) = &test.header.expected_compile_error {
        return Err(format!(
            "expected compile error {code} but compilation succeeded"
        ));
    }
    if let Some(code) = &test.header.expected_warning {
        if !warnings.iter().any(|w| w.code() == code) {
            return Err(format!(
                "expected warning {code} but it was not reported (got: [{}])",
                warnings
                    .iter()
                    .map(|w| w.code())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
    }

    let modules = match nl_codegen::compile_program_with(&files, level) {
        Ok(m) => m,
        Err(e) => return Err(format!("codegen error: {e}")),
    };

    if let Some(msg) = check_module_structure(&test.header, &modules) {
        return Err(msg);
    }

    if test.header.is_compile_only() {
        return Ok(Executed::NotRun);
    }

    if test.header.expected_exit_code.is_none() {
        return Ok(Executed::NotRun);
    }

    if let Err(e) = nl_sema::check_entry_point(&files) {
        return Err(format!("entry point check failed: {e}"));
    }

    if !modules.iter().any(|m| m.find_method("main").is_some()) {
        return Err("no module with 'main' found after codegen".to_string());
    }

    let run_outcome = match &test.header.stdin {
        Some(input) => nl_vm::run_program_with_stdin(&modules, &[], input),
        None => nl_vm::run_program(&modules, &[]),
    };

    if let Some(expected) = test.header.expected_exit_code {
        if run_outcome.exit_code != expected {
            let detail = if run_outcome.stderr.is_empty() {
                String::new()
            } else {
                format!(" ({})", run_outcome.stderr)
            };
            return Err(format!(
                "exit code mismatch: expected {expected}, got {}{detail}",
                run_outcome.exit_code
            ));
        }
    }
    if let Some(expected) = &test.header.expected_stdout {
        if &run_outcome.stdout != expected {
            return Err(format!(
                "stdout mismatch: expected {expected:?}, got {:?}",
                run_outcome.stdout
            ));
        }
    }
    if let Some(expected) = &test.header.expected_stderr {
        if &run_outcome.stderr != expected {
            return Err(format!(
                "stderr mismatch: expected {expected:?}, got {:?}",
                run_outcome.stderr
            ));
        }
    }

    Ok(Executed::Ran(run_outcome))
}

fn check_module_structure(header: &Header, modules: &[Module]) -> Option<String> {
    if let Some(expected_class) = &header.expected_class {
        let found = modules
            .iter()
            .any(|m| m.this_class_name() == Some(expected_class.as_str()));
        if !found {
            return Some(format!(
                "expected_class '{expected_class}' not found in any compiled module"
            ));
        }
    }

    if let Some(expected_methods) = &header.expected_methods {
        for name in expected_methods {
            let found = modules.iter().any(|m| {
                m.methods
                    .iter()
                    .any(|meth| m.constant_pool.utf8_at(meth.name_index) == Some(name.as_str()))
            });
            if !found {
                return Some(format!(
                    "expected method '{name}' not found in any compiled module"
                ));
            }
        }
    }

    if let Some(expected_fields) = &header.expected_fields {
        for entry in expected_fields {
            let (name, ty) = match entry {
                serde_yaml::Value::String(s) => (s.clone(), None),
                serde_yaml::Value::Mapping(map) => {
                    let name = map
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let ty = map
                        .get("type")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    (name, ty)
                }
                _ => continue,
            };
            let found = modules.iter().any(|m| {
                m.fields.iter().any(|f| {
                    let name_matches = m.constant_pool.utf8_at(f.name_index) == Some(name.as_str());
                    let type_matches = ty
                        .as_deref()
                        .is_none_or(|t| m.constant_pool.type_desc_at(f.type_index) == Some(t));
                    name_matches && type_matches
                })
            });
            if !found {
                return Some(format!(
                    "expected field '{name}' not found in any compiled module"
                ));
            }
        }
    }

    if let Some(expected_cp) = &header.expected_constant_pool_contains {
        for needle in expected_cp {
            let found = modules.iter().any(|m| {
                m.constant_pool.entries().iter().any(|e| match e {
                    nl_bytecode::ConstantPoolEntry::Utf8(s) => s == needle,
                    _ => false,
                })
            });
            if !found {
                return Some(format!(
                    "constant pool entry '{needle}' not found in any compiled module"
                ));
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testfile::parse_test_file;

    fn outcome(exit_code: i32, stdout: &str, stderr: &str) -> RunOutcome {
        RunOutcome {
            exit_code,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
        }
    }

    #[test]
    fn identical_runs_do_not_differ() {
        assert!(diff(&outcome(0, "hi", ""), &outcome(0, "hi", "")).is_none());
    }

    #[test]
    fn each_observable_channel_is_compared() {
        let base = outcome(0, "hi", "");
        assert!(diff(&base, &outcome(1, "hi", "")).is_some());
        assert!(diff(&base, &outcome(0, "ho", "")).is_some());
        assert!(diff(&base, &outcome(0, "hi", "boom")).is_some());
    }

    /// End-to-end proof that differential mode can *fail*, not just pass.
    /// Without it, a harness that silently compared nothing would look green
    /// forever — which is exactly the state the suite is in until the first
    /// optimization pass lands.
    #[test]
    fn differential_mode_detects_a_run_that_differs() {
        let test = parse_test_file(&varying_output_fixture("")).expect("fixture must parse");

        match run_test(&test, Mode::Single(OptLevel::O0)) {
            Outcome::Pass => {}
            other => panic!(
                "fixture must pass at a single level: {}",
                match other {
                    Outcome::Fail(reason) => reason,
                    _ => "reported as not compared".to_string(),
                }
            ),
        }

        match run_test(&test, Mode::Differential) {
            Outcome::Fail(reason) => assert!(
                reason.contains("stdout differs"),
                "expected a stdout divergence, got: {reason}"
            ),
            other => panic!(
                "differential mode missed a run-to-run difference: {}",
                match other {
                    Outcome::PassNotCompared => "reported as not compared",
                    _ => "reported as a pass",
                }
            ),
        }
    }

    /// The same fixture, opted out: optimizations.md § Testing requires the
    /// stack-trace and stack-exhaustion cases to be *excluded* from the
    /// comparison rather than counted as failures.
    #[test]
    fn optimization_sensitive_fixtures_are_not_compared() {
        let test = parse_test_file(&varying_output_fixture("optimization_sensitive: true\n"))
            .expect("fixture must parse");

        match run_test(&test, Mode::Differential) {
            Outcome::PassNotCompared => {}
            Outcome::Pass => panic!("an excluded fixture must be reported as not compared"),
            Outcome::Fail(reason) => panic!("an excluded fixture must not fail: {reason}"),
        }
    }

    /// A fixture whose only stated expectation (exit code 0) holds at either
    /// level, but whose output is different on every run — the shape of an
    /// optimization that changed observable behaviour. `Uuid.random()`
    /// rather than a random int: two draws colliding is what would make
    /// these tests flaky, and for a v4 UUID that is not a risk worth writing
    /// a retry for.
    fn varying_output_fixture(extra_header: &str) -> String {
        format!(
            "\
---
expected_exit_code: 0
{extra_header}---

#NLFILE difftest/Main.nl
namespace difftest;
class Main {{
\tpublic static int main(string[] args) {{
\t\tsystem.Out.println(system.Uuid.random());
\t\treturn 0;
\t}}
}}
"
        )
    }
}
