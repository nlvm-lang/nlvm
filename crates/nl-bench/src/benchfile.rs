use anyhow::{bail, Context, Result};
use serde::Deserialize;

use nl_test_runner::fixture::{parse_blocks, split_front_matter, SourceBlock};

/// YAML front matter of a benchmark fixture. Same file format as the `tests/`
/// fixtures (`nl_test_runner::fixture`), different keys: a benchmark says how
/// many times to measure it rather than what diagnostic it expects.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct BenchHeader {
    /// Documentation for whoever reads the fixture — what the numbers mean and
    /// why the program is shaped the way it is. Never printed.
    #[allow(dead_code)]
    pub title: Option<String>,
    pub file_separator: Option<String>,
    /// Measured compilations (source → modules). Defaults to 20: a compile is
    /// a couple of milliseconds, so samples are cheap, and it needs plenty of
    /// them — at that scale the OS's scheduling noise is a large fraction of
    /// the measurement (5 samples gave a 20%+ relative deviation, 20 gives a
    /// few percent).
    pub compile_iterations: Option<u32>,
    /// Measured runs of the compiled program. Defaults to 10.
    pub run_iterations: Option<u32>,
    /// Unmeasured iterations run first, for both phases. Defaults to 1 — just
    /// enough to fault in the pages and warm the allocator; there is no JIT to
    /// warm up.
    pub warmup: Option<u32>,
    /// Sanity check: a benchmark that throws on its first statement is very
    /// fast and completely meaningless, so its output is verified once before
    /// anything is timed.
    pub expected_stdout: Option<String>,
    /// Defaults to 0 — a benchmark is expected to succeed.
    pub expected_exit_code: Option<i32>,
}

impl BenchHeader {
    pub fn file_separator_or_default(&self) -> &str {
        self.file_separator.as_deref().unwrap_or("#NLFILE")
    }
}

pub struct BenchFile {
    /// File stem (`arith_loop.yaml` → `arith_loop`). Identifies the benchmark
    /// in the report and in the baseline file, so renaming a fixture orphans
    /// its baseline entry on purpose.
    pub name: String,
    pub header: BenchHeader,
    pub blocks: Vec<SourceBlock>,
}

pub fn parse_bench_file(name: &str, content: &str) -> Result<BenchFile> {
    let (yaml_str, body_lines) = split_front_matter(content)?;

    let header: BenchHeader = if yaml_str.trim().is_empty() {
        BenchHeader::default()
    } else {
        serde_yaml::from_str(&yaml_str).context("parsing YAML front matter")?
    };

    let separator = header.file_separator_or_default().to_string();
    let blocks = parse_blocks(&body_lines, &separator);
    if blocks.is_empty() {
        bail!("no '{separator}' source block");
    }

    Ok(BenchFile {
        name: name.to_string(),
        header,
        blocks,
    })
}
