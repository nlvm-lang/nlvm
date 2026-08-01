# nlvm

Implementation of the **NL** language, specified in [`nlvm-specs`](https://github.com/nlvm-lang/nlvm-specs): compiler (`nlc`), bytecode virtual machine (`nlvm`), and YAML test runner (`nltest`).

The `nlvm-specs` release currently targeted is tracked in [`SPECS_VERSION`](SPECS_VERSION) (bumped whenever new specs are implemented) and reported by `nlc --version` / `nlvm --version`.

See [CHANGELOG.md](CHANGELOG.md) for a history of notable changes, and [Next.md](Next.md) for open items and implementation notes.

## Example

```
namespace hello;
class Main {
    public static int main(string[] args) {
        system.Out.println("Hello, world!");
        return 0;
    }
}
```

## Structure

```
crates/
├── nl-syntax/       # lexer + parser + AST
├── nl-sema/         # semantic analysis (name resolution, typing, checks)
├── nl-bytecode/     # .nlm module format (shared encoding/decoding)
├── nl-codegen/      # AST -> bytecode
├── nl-vm/           # interpreter (frames, stack, opcodes)
├── nlc/             # compiler CLI binary
├── nlvm/            # VM CLI binary
├── nl-test-runner/  # `nltest` binary, runs YAML tests
└── nl-bench/        # `nlbench` binary, runs YAML benchmarks
```

## Build

```sh
cargo build -r
```

A [`Makefile`](Makefile) wraps the cargo invocations used below — `make` (build), `make test`, `make bench`, `make check` (format + clippy + tests), `make install`. `make help` lists them all. It adds nothing the commands in this README don't do; it just spares you the flags.

## Install

One-liner (downloads the latest prebuilt `nlc`/`nlvm` for Linux x86_64 or macOS arm64 into `~/.local/bin`, which must be on `$PATH`):

```sh
curl -fsSL https://nlvm.dev/install.sh | bash
```

From a clone (builds from source instead, same `~/.local/bin` target — use this on other platforms):

```sh
./install.sh
```

## Usage

```sh
# Compile .nl sources into a single .nlp program (named after the entry class)
nlc -o out/ Main.nl

# ...or to an explicit path
nlc -o out/prog.nlp Main.nl

# Run a compiled program
nlvm out/prog.nlp

# Legacy layout: one .nlm module per class/interface
nlc --emit-modules -o out/ Main.nl
nlvm out/   # loads every .nlm/.nlp under the directory
```

### Optimization level

`nlc` takes `-O0` (default) and `-O1`. `-O0` runs no optimization pass at all and is a supported configuration, not a debug escape hatch: [`optimizations.md` § Optimization levels](https://github.com/nlvm-lang/nlvm-specs/blob/main/docs/optimizations.md#optimization-levels) requires every implementation to support it, since it is the reference side of every comparison a program's portability is checked with.

```sh
nlc -O1 -o out/ Main.nl
nlc -O1 --verbose -o out/ Main.nl   # also lists the passes that ran
```

The level is recorded in each compiled module (`opt_level`, module format version 3) and reported per module by `nlvm -v`, along with the format version:

```sh
nlvm -v out/app.Main.nlp
# nlvm: 21 modules loaded
# nlvm:   app.Main (module v3, -O1)
```

A module in format version 1 or 2 predates the field: its level is reported as `unrecorded`, never as `-O0` — it may well have been optimized. Mixing levels in one program is valid (a prebuilt module needn't match the current build), which is why `-v` reports each module rather than a single summary.

`-O` covers compiler-side optimizations only. The VM-side ones (string interning, superinstructions, inline caching) are chosen by `nlvm` at run time and are not governed by `-O<n>`.

## Tests

This repository ships its own YAML test suite under [`tests/`](tests) (`phase{N}_*.yaml`, one file per language feature), which is what CI runs:

```sh
cargo test --workspace
cargo run -p nl-test-runner -- tests
```

Both at once: `make test`.

The suite also runs **differentially**, compiling and running every fixture at `-O0` and again at `-O1` and requiring identical stdout, stderr and exit code — the regression test [`optimizations.md` § Testing](https://github.com/nlvm-lang/nlvm-specs/blob/main/docs/optimizations.md#testing) mandates, and a CI gate:

```sh
cargo run -p nl-test-runner -- --differential tests   # or: make differential
cargo run -p nl-test-runner -- -O1 tests              # one level only
```

A fixture whose output carries a stack trace, or whose termination depends on stack exhaustion, is the one case § Testing allows to differ between levels (inlining and tail call optimization elide frames); those carry `optimization_sensitive: true` and are excluded from the comparison, though still run and checked at both levels.

The canonical spec suite lives in [`nlvm-specs/tests`](https://github.com/nlvm-lang/nlvm-specs/tree/main/tests) (not in this repository) and can be run the same way:

```sh
cargo run -p nl-test-runner -- /local-path-to/nlvm-specs/tests
```

Each `m{N}_*.yaml` file there corresponds to a milestone from [`nlvm-specs/docs/milestones.md`](https://github.com/nlvm-lang/nlvm-specs/blob/main/docs/milestones.md). See [`nlvm-specs/docs/tests.md`](https://github.com/nlvm-lang/nlvm-specs/blob/main/docs/tests.md) for the format.

## Benchmarks

[`benches/`](benches) holds NL programs exercising one cost centre each (arithmetic loops, string building, virtual dispatch, allocation/GC pressure, exception throw/catch, recursion, compiler throughput), in the same YAML fixture format as the tests. `nlbench` compiles and runs each of them in-process, reporting **compile time and run time separately** — a compiler-side optimization and a VM-side one do not move the same column:

```sh
cargo run --release -p nl-bench -- benches   # or: make bench
```

Numbers are wall-clock milliseconds (median of the measured iterations) and are compared against [`benches/baseline.yaml`](benches/baseline.yaml), which records the host they were measured on — a baseline is only meaningful against a run on the same machine. Always use `--release`; a debug build measures the unoptimized interpreter and refuses to record a baseline.

```sh
cargo run --release -p nl-bench -- --filter dispatch    # one benchmark
cargo run --release -p nl-bench -- --save-baseline      # re-record the baseline
cargo run --release -p nl-bench -- --quick --no-compare # smoke test, no timing claim
```

CI is not gated on timings (they are too noisy for that); it only runs `--quick` so a fixture that stops compiling is caught.

## License

[MIT](LICENSE)
