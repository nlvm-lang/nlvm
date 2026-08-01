# Shortcuts around the cargo invocations documented in README.md.
#
# `--locked` everywhere, like CI (.github/workflows/tests.yml): a build here
# must never silently rewrite Cargo.lock.
CARGO       := cargo
CARGO_FLAGS := --locked

.PHONY: all help build test unit-tests fixtures differential bench fmt fmt-check clippy check clean install

# Every recipe here is a cargo invocation, and cargo already parallelizes
# internally; running two of them at once only makes them queue on cargo's
# build lock with interleaved output. `make -jN` stays serial at this level.
.NOTPARALLEL:

all: build

help:
	@echo "make build         release build of the whole workspace (default target)"
	@echo "make test          cargo test --workspace, then the YAML fixture suite — what CI runs"
	@echo "make unit-tests    Rust unit tests only"
	@echo "make fixtures      YAML fixture suite only (tests/, via nltest)"
	@echo "make differential  fixture suite at -O0 and -O1, outputs compared"
	@echo "make bench         benchmark suite (benches/, via nlbench) — release only, by design"
	@echo "make fmt           cargo fmt --all"
	@echo "make fmt-check     fail if anything is unformatted"
	@echo "make clippy        cargo clippy over the workspace"
	@echo "make check         fmt-check + clippy + test + differential"
	@echo "make clean         cargo clean"
	@echo "make install       ./install.sh (builds from source, links into ~/.local/bin)"

build:
	$(CARGO) build -r $(CARGO_FLAGS)

# The two halves CI runs, in the same order.
test: unit-tests fixtures

unit-tests:
	$(CARGO) test --workspace $(CARGO_FLAGS)

# `build` already produces target/release/nltest; naming that binary as a
# prerequisite too would be a file with no rule to build it, which breaks
# under `make -j`.
fixtures: build
	target/release/nltest tests/

# optimizations.md § Testing — every fixture must behave identically with
# and without optimizations. Separate from `test` because it compiles and
# runs the whole suite twice; `make check` runs it.
differential: build
	target/release/nltest --differential tests/

# Release only: a debug build measures the unoptimized interpreter and nlbench
# refuses to record a baseline from it.
bench: build
	target/release/nlbench benches/

fmt:
	$(CARGO) fmt --all

fmt-check:
	$(CARGO) fmt --all -- --check

clippy:
	$(CARGO) clippy --workspace --all-targets $(CARGO_FLAGS)

check: fmt-check clippy test differential

clean:
	$(CARGO) clean

install:
	./install.sh
