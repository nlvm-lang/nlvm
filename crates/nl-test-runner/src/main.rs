mod header;
mod runner;
mod testfile;

use anyhow::{bail, Context, Result};
use nl_bytecode::OptLevel;

use runner::Mode;

const USAGE: &str = "usage: nltest [options] [test-dir]

    -O0, -O1          compile the fixtures at this optimization level
                      (default: -O0)
    -d, --differential
                      run every fixture at both levels and require identical
                      stdout, stderr and exit code (optimizations.md § Testing)
    -h, --help        print this message";

fn main() -> Result<()> {
    // Defaults to this repo's own internal fixtures (`tests/`, relative to
    // wherever the binary is invoked from) rather than any machine-specific
    // path — the external nlvm-specs suite lives in a sibling repo whose
    // location isn't knowable here; pass it explicitly, e.g.
    // `cargo run -p nl-test-runner -- /path/to/nlvm-specs/tests` (see README).
    let mut dir = None;
    let mut mode = Mode::Single(OptLevel::O0);

    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "-d" | "--differential" => mode = Mode::Differential,
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            opt if opt.starts_with("-O") => {
                let level = opt[2..]
                    .parse::<u8>()
                    .ok()
                    .and_then(OptLevel::from_number)
                    .with_context(|| {
                        format!("unknown optimization level '{opt}' (supported: -O0, -O1)")
                    })?;
                mode = Mode::Single(level);
            }
            other if other.starts_with('-') => bail!("unknown option '{other}'\n\n{USAGE}"),
            other => dir = Some(other.to_string()),
        }
    }

    let dir = dir.unwrap_or_else(|| "tests".to_string());

    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .with_context(|| format!("reading test directory {dir}"))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "yaml"))
        .collect();
    entries.sort();

    match mode {
        Mode::Single(level) => println!("running {} fixtures at {level}", entries.len()),
        Mode::Differential => println!(
            "running {} fixtures differentially (-O0 vs -O1)",
            entries.len()
        ),
    }

    let mut passed = 0;
    let mut failed = 0;
    let mut not_compared = 0;

    for path in &entries {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let content =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;

        let test = match testfile::parse_test_file(&content) {
            Ok(t) => t,
            Err(e) => {
                println!("FAIL {name}: malformed test file: {e}");
                failed += 1;
                continue;
            }
        };

        match runner::run_test(&test, mode) {
            runner::Outcome::Pass => {
                println!("PASS {name}");
                passed += 1;
            }
            runner::Outcome::PassNotCompared => {
                println!("PASS {name} (levels not compared: optimization-sensitive)");
                passed += 1;
                not_compared += 1;
            }
            runner::Outcome::Fail(reason) => {
                println!("FAIL {name}: {reason}");
                failed += 1;
            }
        }
    }

    println!("---");
    println!(
        "{passed} passed, {failed} failed, {} total",
        passed + failed
    );
    if not_compared > 0 {
        println!(
            "{not_compared} excluded from the -O0/-O1 comparison (stack traces, stack exhaustion \
             — optimizations.md § Testing)"
        );
    }

    if failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}
