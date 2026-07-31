mod baseline;
mod benchfile;
mod measure;

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use baseline::{Baseline, Host};
use measure::{BenchResult, Iterations};

const USAGE: &str = "\
usage: nlbench [options] [benchmark-dir | fixture.yaml...]

Runs the NL benchmark fixtures (default: `benches`), reporting compile time and
run time separately, and compares them against the recorded baseline.

Options:
  -f, --filter <substr>   only run benchmarks whose name contains <substr>
      --baseline <path>   baseline file (default: <dir>/baseline.yaml)
      --save-baseline     overwrite the baseline with this run's numbers
      --no-compare        ignore the baseline entirely
      --threshold <pct>   flag a difference above <pct> as a regression (default: 10)
      --fail-on-regression  exit 1 if any benchmark regressed
      --iterations <n>    override every fixture's measured run iterations
      --compile-iterations <n>  override every fixture's measured compile iterations
      --warmup <n>        override every fixture's warmup iterations
  -q, --quick             one compile + three runs, no warmup (smoke test)
      --version           print version and exit
  -h, --help              print this help and exit
";

struct Options {
    paths: Vec<PathBuf>,
    filter: Option<String>,
    baseline_path: Option<PathBuf>,
    save_baseline: bool,
    compare: bool,
    threshold: f64,
    fail_on_regression: bool,
    iterations: Iterations,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            paths: Vec::new(),
            filter: None,
            baseline_path: None,
            save_baseline: false,
            compare: true,
            // 10%, not 1%: these are wall-clock numbers on a machine that is
            // also doing other things. A real optimization moves a benchmark
            // by much more than the noise floor; anything smaller needs more
            // than this harness to be believed.
            threshold: 10.0,
            fail_on_regression: false,
            iterations: Iterations::default(),
        }
    }
}

fn parse_args(args: &[String]) -> Result<Option<Options>> {
    let mut opts = Options::default();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return Ok(None);
            }
            "--version" => {
                println!("nlbench {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "-f" | "--filter" => {
                i += 1;
                opts.filter = Some(
                    args.get(i)
                        .context("missing argument for -f/--filter")?
                        .clone(),
                );
            }
            "--baseline" => {
                i += 1;
                let path = args.get(i).context("missing argument for --baseline")?;
                opts.baseline_path = Some(PathBuf::from(path));
            }
            "--save-baseline" => opts.save_baseline = true,
            "--no-compare" => opts.compare = false,
            "--fail-on-regression" => opts.fail_on_regression = true,
            "--threshold" => {
                i += 1;
                let value = args.get(i).context("missing argument for --threshold")?;
                opts.threshold = value
                    .parse()
                    .with_context(|| format!("--threshold expects a number, got {value:?}"))?;
            }
            "--iterations" => {
                i += 1;
                opts.iterations.run = Some(parse_count(args.get(i), "--iterations")?);
            }
            "--compile-iterations" => {
                i += 1;
                opts.iterations.compile = Some(parse_count(args.get(i), "--compile-iterations")?);
            }
            "--warmup" => {
                i += 1;
                opts.iterations.warmup = Some(parse_count(args.get(i), "--warmup")?);
            }
            // Fills in only what no explicit flag has set, so
            // `--quick --iterations 1` and `--iterations 1 --quick` mean the
            // same thing.
            "-q" | "--quick" => {
                opts.iterations.compile.get_or_insert(1);
                opts.iterations.run.get_or_insert(3);
                opts.iterations.warmup.get_or_insert(0);
            }
            other if other.starts_with('-') => bail!("unknown option {other}\n\n{USAGE}"),
            other => opts.paths.push(PathBuf::from(other)),
        }
        i += 1;
    }
    Ok(Some(opts))
}

fn parse_count(arg: Option<&String>, name: &str) -> Result<u32> {
    let value = arg.with_context(|| format!("missing argument for {name}"))?;
    value
        .parse()
        .with_context(|| format!("{name} expects a positive integer, got {value:?}"))
}

/// Expands the positional arguments into the fixture list: a directory
/// contributes its `*.yaml` files (sorted, so the report order never depends on
/// the filesystem), a file is taken as-is.
fn collect_fixtures(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut fixtures = Vec::new();
    for path in paths {
        if path.is_dir() {
            let mut entries: Vec<PathBuf> = std::fs::read_dir(path)
                .with_context(|| format!("reading benchmark directory {}", path.display()))?
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "yaml"))
                // The baseline lives in the same directory and is not a fixture.
                .filter(|p| p.file_stem().is_some_and(|s| s != "baseline"))
                .collect();
            entries.sort();
            fixtures.extend(entries);
        } else {
            fixtures.push(path.clone());
        }
    }
    Ok(fixtures)
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut opts = match parse_args(&args)? {
        Some(opts) => opts,
        None => return Ok(()),
    };
    if opts.paths.is_empty() {
        opts.paths.push(PathBuf::from("benches"));
    }

    // A debug build measures the *compiler's* debug assertions and unoptimized
    // interpreter loop, i.e. something between 10x and 50x slower with a
    // different shape. Recording that as a baseline would be actively
    // misleading, so it is refused rather than warned about.
    let release = !cfg!(debug_assertions);
    if !release {
        eprintln!(
            "warning: debug build — these numbers are not comparable with a release baseline."
        );
        eprintln!("         run `cargo run --release -p nl-bench -- ...` instead.");
        if opts.save_baseline {
            bail!("refusing to record a baseline from a debug build");
        }
    }

    let baseline_path = opts.baseline_path.clone().unwrap_or_else(|| {
        opts.paths
            .iter()
            .find(|p| p.is_dir())
            .map(|dir| dir.join("baseline.yaml"))
            .unwrap_or_else(|| PathBuf::from("benches/baseline.yaml"))
    });

    let fixtures = collect_fixtures(&opts.paths)?;
    let host = Host::detect();
    let recorded = if opts.compare {
        Baseline::load(&baseline_path)?
    } else {
        None
    };

    let mut results = Vec::new();
    let mut failures = Vec::new();
    for path in &fixtures {
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| path.display().to_string());
        if let Some(filter) = &opts.filter {
            if !name.contains(filter.as_str()) {
                continue;
            }
        }

        let content =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let bench = match benchfile::parse_bench_file(&name, &content) {
            Ok(b) => b,
            Err(e) => {
                failures.push(format!("{name}: malformed benchmark fixture: {e}"));
                continue;
            }
        };

        // Progress on stderr, so a suite that takes minutes shows which
        // benchmark it is stuck in without polluting the report on stdout.
        eprint!("running {name}... ");
        let started = std::time::Instant::now();
        match measure::run_bench(&bench, opts.iterations) {
            Ok(result) => {
                eprintln!("{:.1}s", started.elapsed().as_secs_f64());
                results.push(result)
            }
            Err(e) => {
                eprintln!("error");
                failures.push(format!("{name}: {e}"))
            }
        }
    }

    if results.is_empty() && failures.is_empty() {
        bail!("no benchmark fixture matched");
    }

    report(&results, recorded.as_ref(), &host, &opts, &baseline_path);

    for failure in &failures {
        println!("ERROR {failure}");
    }

    let regressions = match (&recorded, opts.compare) {
        (Some(base), true) => count_regressions(&results, base, opts.threshold),
        _ => 0,
    };

    if opts.save_baseline {
        let new_baseline = Baseline::from_results(&results);
        new_baseline.save(&baseline_path)?;
        println!("baseline written to {}", baseline_path.display());
    }

    if !failures.is_empty() {
        std::process::exit(1);
    }
    if regressions > 0 && opts.fail_on_regression {
        std::process::exit(1);
    }
    Ok(())
}

fn count_regressions(results: &[BenchResult], base: &Baseline, threshold: f64) -> usize {
    results
        .iter()
        .filter(|r| {
            base.benchmarks.get(&r.name).is_some_and(|entry| {
                delta_percent(r.compile.median(), entry.compile_ms) > threshold
                    || delta_percent(r.run.median(), entry.run_ms) > threshold
            })
        })
        .count()
}

/// Positive = slower than the baseline.
fn delta_percent(current: f64, base: f64) -> f64 {
    if base == 0.0 {
        return 0.0;
    }
    (current - base) / base * 100.0
}

fn report(
    results: &[BenchResult],
    base: Option<&Baseline>,
    host: &Host,
    opts: &Options,
    baseline_path: &Path,
) {
    println!("host: {}", host.describe());
    match base {
        Some(base) => {
            println!(
                "baseline: {} (recorded {}, nlvm {})",
                baseline_path.display(),
                base.recorded_at,
                base.nlvm_version
            );
            if base.host.differs_from(host) {
                println!(
                    "  note: baseline was recorded on {} — the `vs base` columns compare\n\
                     \x20       two different machines and mean nothing.",
                    base.host.describe()
                );
            }
        }
        None if opts.compare => println!(
            "baseline: none at {} (run with --save-baseline to record one)",
            baseline_path.display()
        ),
        None => {}
    }
    println!();

    let compare = base.is_some();
    if compare {
        println!(
            "{:<22} {:>10} {:>7} {:>9} {:>10} {:>7} {:>9}",
            "benchmark", "compile", "rsd", "vs base", "run", "rsd", "vs base"
        );
    } else {
        println!(
            "{:<22} {:>10} {:>7} {:>10} {:>7}",
            "benchmark", "compile", "rsd", "run", "rsd"
        );
    }

    for r in results {
        let entry = base.and_then(|b| b.benchmarks.get(&r.name));
        if compare {
            println!(
                "{:<22} {:>10.3} {:>6.1}% {:>9} {:>10.3} {:>6.1}% {:>9}",
                r.name,
                r.compile.median(),
                r.compile.rsd_percent(),
                delta_cell(entry.map(|e| delta_percent(r.compile.median(), e.compile_ms))),
                r.run.median(),
                r.run.rsd_percent(),
                delta_cell(entry.map(|e| delta_percent(r.run.median(), e.run_ms))),
            );
        } else {
            println!(
                "{:<22} {:>10.3} {:>6.1}% {:>10.3} {:>6.1}%",
                r.name,
                r.compile.median(),
                r.compile.rsd_percent(),
                r.run.median(),
                r.run.rsd_percent(),
            );
        }
    }

    println!("---");
    println!(
        "milliseconds, median of the measured iterations (compile = nlc pipeline, run = nlvm)"
    );
    if compare {
        let regressions = count_regressions(
            results,
            base.expect("compare implies a baseline"),
            opts.threshold,
        );
        let new: Vec<&str> = results
            .iter()
            .filter(|r| {
                !base
                    .expect("compare implies a baseline")
                    .benchmarks
                    .contains_key(&r.name)
            })
            .map(|r| r.name.as_str())
            .collect();
        println!(
            "{} benchmarks, {regressions} over the {:.0}% regression threshold",
            results.len(),
            opts.threshold
        );
        if !new.is_empty() {
            println!("not in the baseline yet: {}", new.join(", "));
        }
    } else {
        println!("{} benchmarks", results.len());
    }
}

fn delta_cell(delta: Option<f64>) -> String {
    match delta {
        Some(d) => format!("{d:+.1}%"),
        None => "—".to_string(),
    }
}
