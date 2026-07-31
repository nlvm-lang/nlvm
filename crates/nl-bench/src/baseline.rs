use std::collections::BTreeMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::measure::BenchResult;

/// A recorded run of the whole suite. Committed to the repo so a regression is
/// visible in a diff, and keyed by host because a wall-clock number recorded on
/// one machine says nothing about another — see `Host::differs_from`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Baseline {
    /// UTC date, `YYYY-MM-DD`.
    pub recorded_at: String,
    /// Workspace version the numbers were recorded at.
    pub nlvm_version: String,
    pub host: Host,
    pub benchmarks: BTreeMap<String, Entry>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Host {
    pub os: String,
    pub arch: String,
    pub cpu: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Entry {
    pub compile_ms: f64,
    pub run_ms: f64,
}

impl Host {
    pub fn detect() -> Self {
        Self {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            cpu: detect_cpu(),
        }
    }

    pub fn differs_from(&self, other: &Host) -> bool {
        self != other
    }

    pub fn describe(&self) -> String {
        format!("{} {} ({})", self.os, self.arch, self.cpu)
    }
}

/// Best effort, and deliberately so: the CPU model is a label printed next to
/// the numbers to stop someone comparing a laptop's baseline with a CI runner's,
/// not something the harness reasons about. Anything it cannot read is
/// `"unknown"`, which simply makes the host comparison stricter.
fn detect_cpu() -> String {
    if let Ok(cpuinfo) = std::fs::read_to_string("/proc/cpuinfo") {
        for line in cpuinfo.lines() {
            if let Some((key, value)) = line.split_once(':') {
                if key.trim() == "model name" {
                    return value.trim().to_string();
                }
            }
        }
    }
    if let Ok(out) = std::process::Command::new("sysctl")
        .args(["-n", "machdep.cpu.brand_string"])
        .output()
    {
        let brand = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !brand.is_empty() {
            return brand;
        }
    }
    "unknown".to_string()
}

impl Baseline {
    pub fn from_results(results: &[BenchResult]) -> Self {
        Self {
            recorded_at: today_utc(),
            nlvm_version: env!("CARGO_PKG_VERSION").to_string(),
            host: Host::detect(),
            benchmarks: results
                .iter()
                .map(|r| {
                    (
                        r.name.clone(),
                        Entry {
                            // Three decimals: enough to keep sub-millisecond
                            // phases meaningful, few enough that re-recording
                            // an unchanged suite produces a readable diff
                            // instead of a wall of noise digits.
                            compile_ms: round3(r.compile.median()),
                            run_ms: round3(r.run.median()),
                        },
                    )
                })
                .collect(),
        }
    }

    pub fn load(path: &Path) -> Result<Option<Self>> {
        match std::fs::read_to_string(path) {
            Ok(content) => {
                let baseline = serde_yaml::from_str(&content)
                    .with_context(|| format!("parsing baseline {}", path.display()))?;
                Ok(Some(baseline))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("reading baseline {}", path.display())),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let yaml = serde_yaml::to_string(self).context("serializing baseline")?;
        let header = "# nlbench baseline — regenerate with:\n\
                      #   cargo run --release -p nl-bench -- --save-baseline\n\
                      # Wall-clock milliseconds, median of the measured iterations. Only\n\
                      # comparable against a run on the same host (see `host:`).\n";
        std::fs::write(path, format!("{header}{yaml}"))
            .with_context(|| format!("writing baseline {}", path.display()))
    }
}

fn round3(ms: f64) -> f64 {
    (ms * 1000.0).round() / 1000.0
}

/// `YYYY-MM-DD` from the Unix epoch — Howard Hinnant's civil-from-days, the
/// same conversion `nl_vm::mini_tz` does internally. Written out here rather
/// than pulling in a date crate for one field of one file.
fn today_utc() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let z = secs.div_euclid(86_400) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}
