# nlvm — repères rapides

Implémentation Rust du langage **NL** (specs : [nlvm-lang/nlvm-specs](https://github.com/nlvm-lang/nlvm-specs)).
Détails projet/install/usage → [README.md](README.md). Version specs ciblée → [SPECS_VERSION](SPECS_VERSION).

## Crates (`crates/`)

| Crate | Rôle |
|---|---|
| `nl-syntax` | lexer, parser, AST |
| `nl-sema` | analyse sémantique (résolution, typage, checks E0xx) |
| `nl-bytecode` | format module `.nlm` (encodage/décodage partagé) |
| `nl-codegen` | AST → bytecode |
| `nl-vm` | interpréteur (frames, stack, opcodes, GC) |
| `nlc` | binaire CLI compilateur |
| `nlvm` | binaire CLI VM |
| `nl-test-runner` | binaire `nltest`, exécute les tests YAML de `tests/` ; expose aussi `nl_test_runner::fixture` (parsing du format fixture, partagé avec `nl-bench`) |
| `nl-bench` | binaire `nlbench`, exécute les benchmarks YAML de `benches/` |

## Commandes (`Makefile`)

Raccourcis autour des commandes cargo (tout en `--locked`, comme la CI) : `make` (build release), `make test` (`cargo test --workspace` + suite YAML, ce que fait la CI), `make unit-tests` / `make fixtures` séparément, `make bench`, `make fmt` / `fmt-check` / `clippy`, `make check` (les trois + tests), `make install`. `make help` les liste.

## Tests

`tests/*.yaml` — fixtures organisées par phase (`phaseN_...`). Lancer via `nltest` (`cargo run -p nl-test-runner` ou binaire `nltest`).

## Benchmarks

`benches/*.yaml` — même format de fixture que `tests/`, un programme NL par centre de coût. Lancer en release : `cargo run --release -p nl-bench -- benches`. Temps de compilation et d'exécution rapportés séparément, comparés à `benches/baseline.yaml` (baseline liée à la machine, à re-enregistrer avec `--save-baseline`). Prérequis des optimisations suivies dans l'issue #18.

## Suivi du projet

- État d'avancement / TODO courant → [Next.md](Next.md) (fichier de notes perso de l'utilisateur — ne pas modifier sans demande explicite)
- Historique des changements → [CHANGELOG.md](CHANGELOG.md)
- Décisions/investigations passées détaillées → [journal/](journal/)
- Écarts vs specs suivis comme issues GitHub (labels `spec-gap`, `component:*`, `optimization`) sur nlvm-lang/nlvm

## Maintenance de ce fichier

À tenir à jour par Claude quand la structure change (ajout/suppression de crate, changement d'organisation des tests, etc.) — garder concis, ne pas dupliquer le contenu des fichiers référencés.

## Comportement

Ne commit jamais sauf si explicitement demandé.

## Github

Fetch issues with `gh api repos/nlvm-lang/nlvm/issues/<id>` (instead of deprecated `gh issue view <id>`).