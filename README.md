# nlvm

Implementation of the **NL** language, specified in [`nlvm-specs`](https://github.com/tivins/nlvm-specs): compiler (`nlc`), bytecode virtual machine (`nlvm`), and YAML test runner (`nltest`).

See [PLAN.md](PLAN.md) for the detailed roadmap (phases, decisions, progress tracking).

See [CHANGELOG.md](CHANGELOG.md) for a history of notable changes.

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
└── nl-test-runner/  # `nltest` binary, runs YAML tests
```

## Build

```sh
cargo build -r
```

## Usage

```sh
# Compile a .nl file into an .nlm module
cargo run -p nlc -- -o out/ Main.nl

# Run a compiled module
cargo run -p nlvm -- out/Main.nlm
```

## Tests

The YAML test suite lives in [`nlvm-specs/tests`](https://github.com/tivins/nlvm-specs/tree/main/tests) (not in this repository). The runner executes it directly:

```sh
cargo run -p nl-test-runner -- /local-path-to/nlvm-specs/tests
```

Each `m{N}_*.yaml` file corresponds to a milestone from [`nlvm-specs/docs/milestones.md`](https://github.com/tivins/nlvm-specs/blob/main/docs/milestones.md). See [`nlvm-specs/docs/tests.md`](https://github.com/tivins/nlvm-specs/blobl/main/docs/tests.md) for the format.
