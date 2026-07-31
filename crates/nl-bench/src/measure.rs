use std::hint::black_box;
use std::time::Instant;

use anyhow::{bail, Result};
use nl_bytecode::Module;

use crate::benchfile::BenchFile;

/// CLI overrides for the per-fixture iteration counts. `None` keeps whatever
/// the fixture's front matter asks for.
#[derive(Debug, Clone, Copy, Default)]
pub struct Iterations {
    pub compile: Option<u32>,
    pub run: Option<u32>,
    pub warmup: Option<u32>,
}

/// Timing samples for one phase, in milliseconds.
pub struct Stats {
    samples: Vec<f64>,
}

impl Stats {
    fn new(mut samples: Vec<f64>) -> Self {
        samples.sort_by(|a, b| a.partial_cmp(b).expect("no NaN in a duration"));
        Self { samples }
    }

    /// The headline number. Median rather than mean: a benchmark run that gets
    /// descheduled produces one huge sample, which would drag a mean around
    /// while leaving the median where it belongs.
    pub fn median(&self) -> f64 {
        let n = self.samples.len();
        if n == 0 {
            return 0.0;
        }
        if n % 2 == 1 {
            self.samples[n / 2]
        } else {
            (self.samples[n / 2 - 1] + self.samples[n / 2]) / 2.0
        }
    }

    /// Relative standard deviation, in percent — how much to trust the median.
    /// A comparison against a baseline is only meaningful when the difference
    /// is larger than this.
    pub fn rsd_percent(&self) -> f64 {
        let n = self.samples.len();
        if n < 2 {
            return 0.0;
        }
        let mean = self.samples.iter().sum::<f64>() / n as f64;
        if mean == 0.0 {
            return 0.0;
        }
        let variance = self
            .samples
            .iter()
            .map(|s| (s - mean) * (s - mean))
            .sum::<f64>()
            / (n - 1) as f64;
        variance.sqrt() / mean * 100.0
    }
}

pub struct BenchResult {
    pub name: String,
    /// Source → modules: `nl-syntax` + `nl-sema` + `nl-codegen`. What a
    /// compiler-side optimization (#25, #26) makes slower and a program's run
    /// phase makes faster.
    pub compile: Stats,
    /// Executing the compiled modules: `nl-vm` only, linking included (it is
    /// part of `run_program` and of what a VM-side optimization changes).
    pub run: Stats,
}

/// Compiles the fixture's source blocks from scratch — parsing included, since
/// `nl_sema::check_compile_with_warnings` annotates the AST in place and a
/// second pass over an already-checked AST is not the work we want to measure.
fn compile(bench: &BenchFile) -> Result<Vec<Module>> {
    let mut files = Vec::with_capacity(bench.blocks.len());
    for block in &bench.blocks {
        match nl_syntax::parse_source_file(&block.content, block.path.clone()) {
            Ok(f) => files.push(f),
            Err(e) => bail!("parse error in {}: {e}", block.path),
        }
    }
    if let Err(e) = nl_sema::check_compile_with_warnings(&mut files) {
        bail!("compile error: {e}");
    }
    match nl_codegen::compile_program(&files) {
        Ok(modules) => Ok(modules),
        Err(e) => bail!("codegen error: {e}"),
    }
}

pub fn run_bench(bench: &BenchFile, overrides: Iterations) -> Result<BenchResult> {
    let compile_iterations = overrides
        .compile
        .or(bench.header.compile_iterations)
        .unwrap_or(20);
    let run_iterations = overrides.run.or(bench.header.run_iterations).unwrap_or(10);
    let warmup = overrides.warmup.or(bench.header.warmup).unwrap_or(1);

    // Correctness first: a benchmark whose program throws on line 1 finishes
    // in microseconds and would otherwise be reported as a spectacular win.
    let modules = compile(bench)?;
    let outcome = nl_vm::run_program(&modules, &[]);
    let expected_exit_code = bench.header.expected_exit_code.unwrap_or(0);
    if outcome.exit_code != expected_exit_code {
        let detail = if outcome.stderr.is_empty() {
            String::new()
        } else {
            format!(" ({})", outcome.stderr.trim_end())
        };
        bail!(
            "exit code mismatch: expected {expected_exit_code}, got {}{detail}",
            outcome.exit_code
        );
    }
    if let Some(expected) = &bench.header.expected_stdout {
        if &outcome.stdout != expected {
            bail!(
                "stdout mismatch: expected {expected:?}, got {:?}",
                outcome.stdout
            );
        }
    }

    for _ in 0..warmup {
        black_box(compile(bench)?);
    }
    let mut compile_samples = Vec::with_capacity(compile_iterations as usize);
    for _ in 0..compile_iterations {
        let start = Instant::now();
        let modules = compile(bench)?;
        compile_samples.push(elapsed_ms(start));
        black_box(modules);
    }

    for _ in 0..warmup {
        black_box(nl_vm::run_program(&modules, &[]));
    }
    let mut run_samples = Vec::with_capacity(run_iterations as usize);
    for _ in 0..run_iterations {
        let start = Instant::now();
        let outcome = nl_vm::run_program(&modules, &[]);
        run_samples.push(elapsed_ms(start));
        black_box(outcome);
    }

    Ok(BenchResult {
        name: bench.name.clone(),
        compile: Stats::new(compile_samples),
        run: Stats::new(run_samples),
    })
}

fn elapsed_ms(start: Instant) -> f64 {
    start.elapsed().as_nanos() as f64 / 1_000_000.0
}
