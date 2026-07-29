# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.16.1]

### Fixed
- The cycle collector now reclaims a cycle as soon as the last variable pointing into it is reassigned, instead of waiting for the enclosing function to return. An assignment used as a bare statement no longer produces its own value: `compile_assign` used to keep a copy of the assigned value (for `a = b = 1;` to work) in a compiler-generated scratch local even when nothing consumed it, and that hidden local kept whatever it referenced reachable until the frame returned. Statement-position assignments are now compiled in value-discarding mode, which also emits shorter bytecode (no `DUP`/store/reload triple, one less local slot per field, element or `static` assignment). See [issue #17](https://github.com/nlvm-lang/nlvm/issues/17).
- Values dropped from the *operand stack* — a discarded call result (`POP`), and everything an exception unwind discards on its way to a handler — are now registered as cycle-collector candidates like any other reference drop. Previously only durable slots (field, array element, local, `static`) were, so such a drop relied on a later unrelated event to trigger the pass that would notice it. Operand drops are far too frequent to collect on individually, so they only trigger a pass in batches; a compute-heavy benchmark shows no measurable slowdown. See [issue #17](https://github.com/nlvm-lang/nlvm/issues/17).

### Changed
- The cycle collector no longer runs a pass while a `system.thread.Thread` started by the program is still running. Trial deletion reads reference counts and fields as a series of snapshots, so a concurrent mutator could in principle make a live object look collectible and get its destructor called; passes now wait until the calling thread is provably the only one mutating. Candidates noted meanwhile are not lost — the first pass after the last thread is joined picks them up, as does the end-of-program sweep. A program that abandons a still-running thread and returns from `main` (which vm.md says is not waited for) now exits without reclaiming its cycles. See [issue #17](https://github.com/nlvm-lang/nlvm/issues/17).

## [0.16.0]

### Added
- The VM now performs the **link-time** rejection of `NEW` targeting an `ABSTRACT` class that vm.md § Class flag bits prescribes, instead of only the runtime one. `verify_link` sweeps every loaded method's code array before anything executes (including `<clinit>`) and rejects the program if any `NEW` instruction — reachable or not — names a class flagged `ABSTRACT`; the error names the offending method and class. Previously a `NEW` sitting in a never-called method or a branch that never runs went undetected, and a program containing one exited `0`. `nl-sema`'s `E032` already rejects this at compile time, so this only changes behaviour for bytecode that reached the VM without it (a hand-written or third-party `.nlm`, or an in-memory program built by an embedder). `Opcode::New` keeps its runtime check as the spec's own fallback. See [issue #16](https://github.com/nlvm-lang/nlvm/issues/16).
- `nl-bytecode` gained a `disasm` module (`instructions(code)`) that decodes a code array into instructions, plus `Opcode::operand_len` — the shared, reusable basis for the sweep above and for any future bytecode-walking tool (disassembler, static analysis).

## [0.15.0]

### Added
- `++`/`--` (prefix and postfix) now accept any assignable target, not just a plain variable name: `obj.field++`, `this.field++`, `Class.staticField++` and `arr[i]++` all work, as do their `--` and prefix counterparts. Previously only a bare identifier parsed at all (`obj.field++` was a parse error), because the AST modelled the operand as a `String`; it is now the same `LValue` the left-hand side of `=` uses, so field/element targets get the same write-permission checks as an assignment (`E010` inside a `const` method, `E014` on a `readonly` property, member accessibility) and the same value rule as before (`int`, or a type overloading `operator++`/`operator--`, otherwise `E009` — now reported by nl-sema for field/element targets instead of surfacing as an opaque nl-codegen error). Codegen evaluates the target's sub-expressions exactly once, so a side-effecting receiver or index (`a[i++]--`) behaves as written. See [issue #10](https://github.com/nlvm-lang/nlvm/issues/10).

## [0.14.4]

### Fixed
- Closure target typing (specs.md § Return type deduction rules, point 5) is now implemented: a closure literal assigned directly to an explicitly typed function-type local, with its own return type omitted, is now checked and compiled against the target's return type instead of its own body-deduced one — e.g. `(int) => float k = (int n) => n;` now compiles (`int` widens to `float`) instead of failing with `E004`-eluding leniency at nl-sema followed by a hard `cannot assign Closure { ... }` error at nl-codegen (the closure's own deduced `(int) => int` never matched the target's `(int) => float` descriptor). This covers both an expression body (`(int n) => n` — the grammar has no slot to write an explicit return type there at all) and a block body with omitted return type; `nl-sema`'s `check_closure` and `nl-codegen`'s `compile_closure`/`Emitter::expected_return_ty` now both thread the target's return type into the closure literal instead of leaving it as a "no dedicated type" leniency case. See [issue #14](https://github.com/nlvm-lang/nlvm/issues/14).

## [0.14.3]

### Fixed
- A mutated closure capture is now boxed even when the captured local is declared with `auto` rather than an explicit type (e.g. `auto n = 0; auto inc = () => { n++; };`). Previously this crashed the compiler outright (`boxed captures are always explicitly typed`): `nl_syntax::monomorphize`'s `Box<T>` instantiation pass runs on bare AST before any type checking, so it could only ever recover a concrete type for an *explicitly*-typed declaration, while nl-codegen's (type-blind) capture analysis still expected every such capture to be boxed. `nl-sema` now detects this exact case — an `auto` local that's captured by a closure and mutated (inside the closure or by the enclosing scope) anywhere in the method — using the type it already resolves for the declaration, and patches that type back into the caller's own AST before nl-codegen ever runs, so nl-codegen needs no change at all. The shared "captured ∩ mutated" name analysis (previously duplicated only in `nl-codegen`) moved to `nl_syntax::capture` so `nl-sema` can reuse it without a cross-crate dependency. A template method where two instantiations disagree on the concrete type of such a variable (e.g. `Holder<int>` vs. `Holder<string>`) is a known follow-up gap: patching the shared template source would silently miscompile whichever instantiation didn't "win", so that specific combination is detected and left unfixed, deterministically falling back to the pre-existing compiler crash rather than risk a wrong answer. See [issue #15](https://github.com/nlvm-lang/nlvm/issues/15).

## [0.14.2]

### Fixed
- `Self` in an interface method (e.g. `Cloneable.clone()`) now works when called through an **interface-typed** receiver (`Cloneable c = new Point(...); c.clone()`), not just a concrete-typed one. Previously this crashed at runtime with `method '<Class>.<method>' not found` — even on code with no further use of the result — because the call site's method descriptor was built from the interface's own declaration (return type `Self`, unresolved), which can never match the concrete implementing class's own descriptor (each implementer resolves `Self` to itself, e.g. `Point.clone(): Point`), and virtual dispatch matched on the full descriptor including return type. Instance-method dispatch now matches on name and parameter types only (return-type-only overloading isn't supported by this language, so this can't introduce ambiguity); separately, the static type of such a call now resolves `Self` to the receiver's own static type (the interface) instead of leaking the internal `Self` placeholder, so further use of the result (an explicit downcast to recover the concrete type, assignment, etc.) type-checks correctly instead of surfacing that placeholder in an unrelated error. See [issue #11](https://github.com/nlvm-lang/nlvm/issues/11).

## [0.14.1]

### Fixed
- `system.io.File.glob` now supports real glob syntax (`*`, `**`, `?`, `[...]`), not just regex. Previously every pattern was compiled as a regex, so a plain glob like `"*.txt"` matched only the literal string `*.txt` (`*` had nothing to quantify, so it was parsed as a literal character) instead of files ending in `.txt`. A pattern is now compiled as a regex only if it uses regex-only syntax (backslash escapes, `^`/`$` anchors, `|` alternation, `+`, groups, `{m,n}`) — this keeps existing regex patterns (e.g. `".*\\.nl"`) working unchanged — and as a glob otherwise, via a new `mini_regex::compile_glob` translator (`*` → any run of non-`/` characters, `**` → any run of characters including `/`, `?` → one non-`/` character). See [issue #3](https://github.com/nlvm-lang/nlvm/issues/3).

## [0.14.0]

### Added
- `Exception.stackTrace` entries (`ExecutionPoint`) now carry a `methodName` field alongside `file`/`line`, populated from the VM's shadow call stack — an nlvm extension beyond specs.md's `{line, file}`. See [issue #13](https://github.com/nlvm-lang/nlvm/issues/13).

### Fixed
- An expression-body closure (`() => expr`, as opposed to a block-body `() => { ... }`) now records a line-number table entry for its `invoke` method. Previously it had none at all, so any stack trace frame pointing into such a closure always reported line `0`. See [issue #13](https://github.com/nlvm-lang/nlvm/issues/13).

## [0.13.0]

### Added
- `system.In.readLine` now has test coverage: the internal test-runner YAML format gained a `stdin` header key that scripts stdin input line-by-line (EOF once exhausted), so a *run* test can exercise `readLine` without a real pipe. `nl_vm::run_program_with_stdin` backs this — `run_program` itself is unchanged and still reads the real process stdin. See [issue #6](https://github.com/nlvm-lang/nlvm/issues/6).

## [0.12.5]

### Fixed
- Diamond merge of inherited interface declarations is now checked (`E041`). Per specs.md § Interface inheritance, when the same method (name + parameter types) is reachable from several interfaces in an `extends`/`implements` closure, the declarations must agree on return type to merge; disagreeing return types is now rejected as `E041`, the same error code as an ordinary duplicate method declaration. Previously an interface extending two parents that each declared, say, `value()` with a different return type — or a class implementing two such unrelated interfaces — compiled successfully with no diagnostic at all. The check runs before `E033`/`E044` so a structurally-conflicting hierarchy is reported for what it is, not misattributed to a missing implementation. See [issue #9](https://github.com/nlvm-lang/nlvm/issues/9).

## [0.12.4]

### Fixed
- Interface conformance checks (E033 for missing implementations, E044 for `const`-correctness on `implements`) now match an interface method against a class-side implementation by **exact parameter types and exact return type**, not just by name and arity. Previously a class implementing `Handler { void handle(int code); }` with `handle(string code)` — same name, same arity but a different parameter type — silently satisfied the interface; likewise `Reader { string read(); }` was considered implemented by `int read()`. The check now uses the same type-based resolution already applied to `new`/delegation/method calls, walks the whole `extends` chain (so an implementation inherited from a superclass still counts), and treats an interface method's `Self`-typed parameter or return type as the implementing class's own FQCN (matching specs.md § Self in interfaces). E044 also stops firing when the "matching" method is really a different overload (no exact-signature impl exists): a single fixture no longer trips E044 and E033 for the same root cause. See [issue #8](https://github.com/nlvm-lang/nlvm/issues/8).

## [0.12.3]

### Fixed
- Implicit same-class static calls (`foo(x)` without a receiver, resolved against the current class's own static methods) now pick the correct overload by argument type instead of always resolving to whichever same-name overload was declared last. Previously a class declaring two static methods `show(int)` and `show(string)` silently kept only the last declared one in the per-name signature table, so `show(42)` and `show("hi")` both resolved to `show(string)` — the first call failing to compile with `E004` or, worse, targeting the wrong method. Same argument-type-based resolution as the Phase 5 fix already applied to `new T(...)`, `this(...)`/`super(...)` delegation, and dotted method calls (see [issue #7](https://github.com/nlvm-lang/nlvm/issues/7)).

## [0.12.2]

### Fixed
- Objects that only reference each other (a reference cycle — e.g. two objects each holding a field pointing back at the other, or a self-referencing object) are now reclaimed and have their `destruct()` called, closing a previously-documented gap where the `Arc`-refcounting GC could never collect them because their reference count never reached zero. A synchronous trial-deletion pass runs alongside the existing refcounting whenever a reference is dropped from a field, array element, local variable, or `static` field, and again once at program exit; it only reclaims a group once no reference to any of its members exists from outside the group, so an object still reachable through a live variable or a `static` field is never collected while reachable. Collection isn't always instantaneous — reassigning every variable that pointed into a cycle may not free it until the enclosing function returns, a documented limitation — but a cycle is always eventually collected, at the latest by the time the program exits.

## [0.12.1]

### Fixed
- The bytecode `ABSTRACT`/`FINAL` flags (`class_flags`/`method_flags`) are now actually emitted by the compiler and enforced by the VM, closing a gap where they were defined in the module format but never written or checked. An abstract method (interface methods included) is now compiled to a proper code-less stub (`code_length = 0`, no locals/stack/exception/line table) instead of being silently omitted from the module; `abstract`/`final` classes and `final` methods now carry their flag in the compiled bytecode. Loading a module now rejects one with `ABSTRACT`+`FINAL` set together (class or method) or an abstract method with non-empty code — previously undetected malformed bytecode. Loading a program now also rejects a class extending a `final` class or overriding a `final` method, and instantiating an `abstract` class is rejected at runtime as a VM-level safety net — protections that previously existed only at compile time (E032/E035/E036) and gave no defense for a hand-written or corrupted `.nlm`.

## [0.12.0]

### Added
- Prefix `++`/`--` (`++x`, `--x`) is now parsed and compiled — previously only the postfix forms existed. Both prefix and postfix are now real expressions with the value specs.md § Operator precedence documents: postfix evaluates to the pre-mutation value, prefix to the post-mutation one, for a plain `int` local, a `ref int` parameter, and a closure-captured-and-mutated `int` local alike. An overloaded `operator++`/`operator--` on an object still follows specs.md's "Postfix note": both forms evaluate to the same mutated object reference. The target is still restricted to a plain variable name (`obj.field++`/array-element `++`/`--` remain unsupported); `++`/`--` applied to anything else is now a clear parse-time error instead of silently misparsing or failing deep in codegen.

### Fixed
- Method/constructor overload resolution now considers argument types, not just argument count. Previously, when two overloads of the same method or constructor shared the same arity (e.g. `show(int)` and `show(string)`), the compiler always picked whichever was declared first — regardless of the actual argument's type — so calling the "wrong-positioned" overload either failed to compile with a confusing type error or, in rarer cases, silently compiled a call against the wrong method. Resolution now scores each arity-compatible candidate by how well its parameters match the call's argument types (exact match, then numeric widening/subclass/interface compatibility) and picks the best match; ties still fall back to the first declared, a documented limitation. Covers `new T(...)`, `this(...)`/`super(...)` constructor delegation (specs.md § Constructor chaining), and instance/static method calls.
- `this(...)` constructor-delegation cycle detection (E046) also resolved its target by arity only, which could report a false cycle for a same-arity overloaded constructor whose `this(...)` argument was a literal or one of the constructor's own parameters (the delegation target looked like it delegated to itself). It now uses the same argument-type-aware resolution for that common case.

## [0.11.2]

### Fixed
- A non-abstract class that implements an interface (directly, via an ancestor `extends`, or transitively through `interface ... extends ...`) without providing all of its methods is now rejected with `E033 — Class must be declared abstract`, matching specs.md § Interface inheritance ("a class implementing an interface must implement all methods") and § Abstract classes and methods ("interface methods are implicitly abstract"). Previously such a class compiled successfully and only failed at runtime, with an unhelpful error, the first time virtual dispatch tried to find the missing method.
- `nl-test-runner` no longer defaults to a machine-specific absolute path (`/data/projects/nlvm-specs/tests`) when run without an argument; it now defaults to this repo's own `tests/` directory, which always exists relative to the invocation.

## [0.11.1]

### Fixed
- `obj++`/`obj--` on a type with no matching `operator++`/`operator--` overload (or a non-`int` primitive, e.g. `string`) now reports `E009` at compile time instead of silently passing semantic analysis and failing later with an unstructured codegen error.
- `system.Out.print`/`println`/`system.Err.print`/`println` now accept an argument whose static type implements `Stringable`, calling its `toString()` by virtual dispatch — matching the `+` concatenation and `(string)` cast behavior (specs.md § Stringable interface lists all three as consumers). Previously rejected at compile time with `E004`.

## [0.11.0]

### Added
- `typedef Type Name;` (specs.md § Typedef): namespace-scoped compile-time type aliases, usable anywhere a type is expected — including as a `new` target for a template alias (`typedef Vector<int> IntVector; new IntVector(...)`) and for function-type aliases (`typedef (int, int) => int BinaryOperation;`). Fully erased before semantic analysis/codegen, so aliases are completely interchangeable with their underlying type.
- `switch`/`case`/`default` statement (specs.md § Switch/Match) with C-like fall-through semantics: execution continues into the next `case` without an explicit `break`. `break` exits the nearest `switch` or loop; `continue` inside a `switch` still targets the nearest enclosing loop.
- `interface A extends B, C` (interface inheritance, compiler.md § Interface inheritance): an interface may extend any number of parent interfaces, inheriting all their method declarations. `instanceof`/upcast and the const-correctness check (E044) both now work transitively through the whole interface hierarchy, not just directly-implemented interfaces.
- `for (const auto item : collection)` — an explicit `const` on a for-each loop variable is now enforced (E039), independent of whether the iterated collection is itself read-only.

### Fixed
- Assigning an object to an interface-typed variable/field/parameter (`Disposable d = someCloseable;`) now correctly recognizes an interface implemented indirectly through another implemented interface's own `extends` chain — previously only directly-`implements`-ed interfaces were recognized at assignment sites (E004), even though `instanceof` already handled deeper class hierarchies correctly.
- Using a reserved keyword where an identifier is expected now reports the documented `E030` message instead of a generic parse error.

## [0.10.0]

### Added
- `static` fields on ordinary (non-enum) classes are now backed by real per-class storage (`GET_STATIC`/`SET_STATIC`), previously unimplemented (`VmError::Unsupported`). A declared initializer (`public static int counter = 0;`) runs once at program load time, before `main`. Accessed and assigned via `ClassName.field`, including through a subclass name when the field is inherited.
- `system.Map`/`system.List` key/element lookup (`get`/`set`/`remove`/`has`/`contains`) now calls a `ValueEquatable`-implementing type's `valueEquals` for structural equality instead of always falling back to reference identity.
- `Stringable.toString()` is now called implicitly by string concatenation (`+`) and the `(string)` cast when an operand's static type implements `Stringable`, instead of being rejected at compile time (E008/E007).

### Fixed
- A class property declared with an inline initializer (`public int x = 42;`) is now actually assigned that value at construction — previously the initializer expression was parsed and accepted by the compiler but silently never applied, leaving the field at its type's plain default (`0`/`""`/`false`/`null`) regardless of what was written.

## [0.9.0]

### Added
- `Self`/`type` contextual keywords are now also usable inside an **interface body** (specs.md § Self in interfaces), e.g. `interface Spawner { public Self spawn(); }` — previously a parse error. An implementing class writes `Self`/`type` (or its own name) in its own method declaration to get the covariant return type; this compiler doesn't verify interface method conformance beyond existing const-correctness checking (E044), so covariance itself relies on the implementer writing the signature correctly, same as before this change.
- Built-in `Cloneable` interface (specs.md § Cloneable interface): `public Self clone();`. Implement it and call `clone()` as an ordinary instance method for a shallow copy.
- Built-in `ValueEquatable` interface (specs.md § ValueEquatable interface): `public bool valueEquals(const Self|null other); public int valueHash();`. Implement it for structural equality distinct from `==` (reference identity). `system.Map`/`List` key lookup does not yet call into `valueEquals`/`valueHash` (still reference identity for object keys) — see `Next.md`.

## [0.8.0]

### Added
- Operator overloading (specs.md § Operator Overloading): classes can now define `operator+ operator- operator* operator/ operator%`, compound assignment (`operator+= operator-= operator*= operator/= operator%=`), comparisons (`operator< operator> operator<= operator>=`), three-way comparison (`operator<=>`), unary `operator-`/`operator!`, and `operator++`/`operator--`. Resolved by exact parameter type, so a class can overload the same operator for several parameter types (e.g. `operator+` for both another instance and `int`) without ambiguity.
- `type`/`Self` contextual return-type keywords (specs.md § Self and type keywords) inside a class/enum body — `type` for methods that construct and return a new instance of the enclosing class (including `new type(...)`), `Self` for methods that mutate and return `this`. (Not yet supported inside interface bodies — see `Next.md`.)

## [0.7.1]

### Fixed
- `+` between two field-access expressions of static type `string` (e.g. `page.root + item.href`), with no string literal or local variable anywhere in the chain to anchor the fast path, no longer fails codegen with "unsupported construct: arithmetic/comparison between StringT and StringT". String concatenation's static-type peek now also resolves through field accesses and method calls, not just literals and local variables.

## [0.7.0]

### Added
- `Exception.printStackTrace()` (specs.md § Exception class hierarchy, v0.8.47) — writes `message` to `system.Err`, followed by one `"    at " + file + ":" + line` line per `stackTrace` frame, in throw-site-first order. Implemented as an ordinary inherited method on the prelude's root `Exception` class, so every built-in and user-defined exception subclass gets it for free.

## [0.6.0]

### Added
- `nodiscard` method modifier (specs.md § Nodiscard) — previously a parse error whenever used. Calling a `nodiscard` method and discarding its return value as a bare statement now reports warning `W001` (compiler.md § Warnings) instead of failing compilation. `nlc` prints reported warnings to stderr without aborting the build.

## [0.5.10]

### Added
- One-line install: `curl -fsSL https://nlvm.dev/install.sh | bash` downloads the latest prebuilt `nlc`/`nlvm` (Linux x86_64, macOS arm64) and verifies it against a published `SHA256SUMS`. Running `./install.sh` from a clone still builds from source, unchanged.
- `release.yml` now generates and publishes a `SHA256SUMS` file alongside each release's binary tarballs.

## [0.5.9]

### Added
- GitHub Actions workflow (`release.yml`) that builds `nlc`/`nlvm` release binaries for Linux and macOS (Intel + Apple Silicon) and publishes them as a GitHub Release on version tags, laying the groundwork for a one-line install script.

### Changed
- Project moved to the `nlvm-lang` GitHub organization; README and VS Code extension links updated accordingly. The documentation site moved to its own `nlvm.dev` repository.

## [0.5.8]

### Fixed
- A closure nested two or more levels deep, referencing a variable captured by an enclosing closure (rather than its own parameters/locals), now compiles instead of failing with "undefined variable" — the capture is correctly re-propagated (including its shared box, if mutated) through every level of nesting.

## [0.5.7]

Closures now capture variables by reference, matching specs.md § Variable capture.

### Fixed
- Anonymous functions capturing a variable that's mutated after capture — either by the enclosing scope or by the closure itself (`counter++` inside the closure body) — now see/produce the same shared value instead of a stale snapshot taken at closure-creation time.

## [0.5.6]

Website & branding: logo assets. No toolchain changes.

### Added
- Brand assets under `docs/assets/brand/`: a master `logo.svg` (the "nl" glyph as drawn paths, no font dependency) and PNG exports from 16 to 1024 px, ready for the future `nlvm-lang` GitHub organization avatar.
- A 1280×640 social preview card (`social-preview.svg` + rendered PNG) for GitHub social previews and Open Graph, using JetBrains Mono / Inter to match the site's own `--mono` / `--sans` type system instead of generic fallback fonts.
- `docs/assets/brand/generate.py`, a single script that builds all of the above from one set of constants — regenerate everything with `python3 generate.py`.

### Changed
- The site favicon now uses `logo.svg` instead of the inline font-dependent data URI, so it renders identically everywhere.
- The header wordmark (all pages) and the home hero brand now display `logo.svg` instead of the CSS-styled text glyph.

## [0.5.5]

Website: home page identity pass. No toolchain changes.

### Changed
- Landing-page hero is now asymmetric: copy and CTAs on the left, the animated terminal demo on the right, so the compiler is on screen from the first second.
- Section kickers are numbered (`01 · Why NL` …) and the footer states how the site is built (hand-written HTML & CSS, no framework, no tracking).

### Added
- A subtle film-grain overlay across the site, a blinking caret in the header wordmark, and a "Devlog" section on the home page surfacing the three latest posts.

## [0.5.4]

Website: hero brand lockup. No toolchain changes.

### Added
- Landing page opens with an NL brand lockup (glyph + "The NL programming language" eyebrow) above the headline, so the language is named at first glance.

## [0.5.3]

Website: interactive terminal demo. No toolchain changes.

### Added
- Landing-page terminal now cycles through four real captured scenarios (build & run, compile checks, stack traces, spec & tests) with clickable tabs to pick one.

## [0.5.2]

Diagnostic formatting fix. No language changes.

### Fixed
- `nlc` no longer prints the compile-error code twice (e.g. `E003 — … (E003)`); the code now appears exactly once, matching `nlc --lint` output. The same duplication is removed from `nl-test-runner` failure messages.

## [0.5.1]

Project website. No toolchain changes.

### Added
- Project website under `docs/` (served by GitHub Pages from `main`/`docs`): landing page, language tour, getting-started guide, and an English devlog — static HTML/CSS/JS, dark theme.

### Changed
- Build journals moved from `docs/` to `journal/` (`docs/` is now the website root); links updated in `CHANGELOG.md` and `Next.md`.

## [0.5.0]

Track the `nlvm-specs` baseline explicitly.

### Added
- `SPECS_VERSION`: single source of truth for the `nlvm-specs` release this implementation targets, bumped whenever new specs are implemented.
- `nlc --version`, reporting the crate version and the tracked `nlvm-specs` version (`nlc` had no version flag before).

### Changed
- `nlvm --version` now also reports the tracked `nlvm-specs` version alongside the crate version.
- `tools/Release.nl` now tags releases as `<version>+<specs version>` (e.g. `0.5.0+0.8.44`) instead of the changelog version alone.

## [0.4.0]

Single-file program output: `.nlp` container format.

### Added
- `.nlp` program container format (`nl-bytecode::program`): one file bundling every module of a compiled program, each embedded as a complete `.nlm` image.
- `nlvm` runs `.nlp` files (and still accepts `.nlm` files, directories, or a mix); containers are detected by magic number, not extension.
- `nlc --emit-modules`: opt back into the previous one-`.nlm`-per-class output layout.

### Changed
- `nlc` now produces a single `.nlp` program by default — `-o` may name the file directly (`-o prog.nlp`) or a directory, in which case the file is named after the entry class (the one defining a static `main`).
- `nlvm --version` reports the real crate version instead of a hardcoded string.

## [0.3.0]

Release helper script written in NL itself.

### Added
- `tools/Release.nl`: reads the latest version from `CHANGELOG.md` and runs `git tag -a`/`git push` for it, demonstrating `system.io.File`, `system.text.Regex`, and `system.ps.Process` together in a real script.

## [0.2.0]

Explicit function type declarations.

### Added
- Explicit function types (`(int) => bool`, with optional `throws`) usable as a variable/field/parameter/return type, per specs.md § Function type assignment.

### Fixed
- `nl-vm`'s descriptor param-count parsing (`count_params`) miscounted a parameter whose own descriptor contains a comma (a function-type parameter, or a mangled generic like `system.Map<K, V>`) — now depth-aware.
- A closure literal with a union-typed parameter (e.g. `string|null`), called through a bare identifier, crashed at runtime (`invoke` not found) — its synthesized `invoke` method's descriptor is now built consistently with what every call site expects.

## [0.1.1]

Stack trace support. Detailed build journal in [journal/journal_02_stack_trace.md](journal/journal_02_stack_trace.md).

### Added
- Exception stack trace capture.
- `StackOverflowException` via call depth limit.
- Shadow stack for stack traces.
- Line-number table in codegen.

## [0.1.0]

Initial implementation of the NL language: compiler (`nlc`), bytecode VM (`nlvm`), and YAML test runner (`nltest`). Detailed build journal in [journal/journal_01_initial_build.md](journal/journal_01_initial_build.md).

### Added
- Lexer, parser, AST, and a shared `.nlm` bytecode format (`nl-bytecode`) between compiler and VM.
- Core semantics: typing, name resolution, null safety, definite assignment, smart-cast narrowing.
- Objects, arrays, interfaces, virtual dispatch, single inheritance.
- Exceptions (`throw`/`try`/`catch`/`finally`), checked-exception verification (E015-E017), `match` expressions.
- Closures (capture by value) and generic classes via monomorphization.
- Full `system.*` standard library: `Out`/`Err`/`In`, `String`, `List<T>`/`Map<K,V>`, `system.io.*` (files, directories, paths, `Grep`), `Random`/`SecureRandom`/`Uuid`, `system.net.*` (TCP/UDP/HTTP with TLS), `system.thread.*` (real OS threads, `Mutex`, `Semaphore`), `system.ps.Process`, `system.text.Regex`/`Encoding`, `system.time.DateTime`/`TimeZone`, `system.Env`.
- Array callback methods (`map`/`filter`/`forEach`/`sort`/`find`/`slice`), array initializer lists, ternary/nullish-coalescing/elvis/three-way-comparison (`<=>`) operators, explicit casts, ref parameters, named/optional/default arguments, readonly enforcement, abstract/final classes, enums.
- Reference-counting GC with destructor calls (`<destruct>`) on last-reference drop.
- Full semantic error-code coverage (49/49 checks from the spec).

### Notes
- Older, phase-by-phase history: `git log` or [journal/journal_01_initial_build.md](journal/journal_01_initial_build.md).
