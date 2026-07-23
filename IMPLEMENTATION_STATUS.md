# État d'implémentation vs. nlvm-specs

Photographie de l'écart entre les spécifications (`nlvm-specs`, `SPECS_VERSION` = 0.8.47 côté ce dépôt) et l'implémentation Rust actuelle (nlvm v0.10.0). Établi par lecture croisée de `specs.md`, `compiler.md`, `vm.md`, `stdlib.md`, `optimizations.md`, `tests.md` contre `crates/nl-syntax`, `crates/nl-sema`, `crates/nl-codegen`, `crates/nl-bytecode`, `crates/nl-vm`, `crates/nl-test-runner`, `Next.md` et `journal/`.

Ne liste que les écarts. Tout ce qui n'apparaît pas ici a été vérifié conforme.

## Résumé

Les milestones 1 à 8 (lexer/parser, sémantique, bytecode, VM core, objets, exceptions/closures, stdlib, test runner) sont **globalement en place et fonctionnels** — 14/14 tests officiels des specs passent, 161/161 tests internes passent (157 + 4 ajoutés en Phase 1, un par écart résolu). Le milestone 9 (optimisations) est entièrement optionnel et n'a pas été commencé, comme prévu par le plan des milestones.

Les trois écarts à fort impact précédemment identifiés (`static` runtime, `ValueEquatable` dans Map/List, `Stringable` dans la concaténation/cast) ont été traités — voir § Phase 1 ci-dessous. Il reste plusieurs écarts de moindre impact (§ 2-6).

---

## Phase 1 — Écarts à fort impact : résolus

Les trois écarts qui **compilaient et semblaient marcher** mais se comportaient silencieusement autrement que documenté ont été câblés. 157/157 tests internes + 14/14 tests officiels des specs passent toujours après ces changements (aucune régression détectée). Quatre fixtures YAML ont été ajoutées à `tests/` (`phase10_0040` à `phase10_0070`, une par écart résolu plus une pour le bug d'initialiseur d'instance ci-dessous) — chacune vérifiée pour échouer contre le code d'avant cette passe (rétabli temporairement via `git stash`) avant d'être validée contre le code corrigé, pour confirmer qu'elle couvre bien le comportement changé et pas autre chose.

### Champs `static` sur classes non-enum
`GET_STATIC`/`SET_STATIC` sont maintenant implémentés. `nl_vm::Program` porte une table de stockage statique par classe (`HashMap<fqcn, HashMap<field_name, Value>>`), pré-remplie avec la valeur par défaut de chaque champ `static` au chargement (`Program::new`). Un champ déclaré avec un initialiseur (`public static int counter = 0;`) est assigné une seule fois, avant `main`, par une méthode synthétique `<clinit>` que `nl-codegen` génère automatiquement (nom réservé, jamais en collision avec du code utilisateur) et que `nl_vm::program::run_static_initializers` exécute pour chaque classe chargée, dans l'ordre de chargement (déterministe, mais pas d'initialisation paresseuse à la Java — documenté comme simplification, cohérent avec le reste de l'implémentation). L'accès `ClassName.field` (lecture et écriture) résout la classe *déclarante* du champ même quand on y accède via une sous-classe (`find_field_owner`, côté `nl-sema` et `nl-codegen`), pour que le stockage soit partagé correctement en cas d'héritage.

**Problème rencontré pendant le développement** : en creusant l'exécution des initialiseurs statiques, il est apparu que les initialiseurs de champs *d'instance* (`public int x = 42;`) n'étaient eux non plus **jamais appliqués** — un champ non-static avec initialiseur gardait silencieusement la valeur par défaut de son type après `new`, quel que soit ce qui était écrit dans le code source. Ce n'était pas l'un des trois écarts listés, mais un bug préexistant plus large découvert en cherchant où (et si) un mécanisme équivalent existait déjà pour les champs static — aucun des 157 tests internes ni des 14 tests officiels n'exerçait ce cas assez précisément pour le détecter (confirmé par un test manuel avant/après : `new Foo().getX()` retournait `0` au lieu de `42`).
**Solution** : `nl-codegen` désucre maintenant tout champ avec initialiseur (statique ou non) en une assignation ordinaire (`this.field = init;` / `ClassName.field = init;`), injectée soit au début de chaque `construct` (juste après un éventuel appel `super(...)`, jamais dupliquée dans un `construct` qui délègue via `this(...)`), soit dans le `<clinit>` synthétique. Corrigé dans le même changement puisque static et instance partagent exactement le même désucrage.

### `ValueEquatable` dans `system.Map`/`system.List`
`get`/`set`/`remove`/`has` (Map) et `contains` (List) appellent maintenant `valueEquals` (dispatch virtuel, via `nl_vm::native::equatable_equals`) quand le type de la clé/élément implémente `ValueEquatable`, au lieu de toujours retomber sur l'identité de référence. `valueHash` reste déclarable et appelable comme une méthode ordinaire mais n'est toujours pas réellement utilisé pour le hachage : `Map<K,V>` reste un stockage par tableaux parallèles à recherche O(n) (changement hors scope — refonte de la structure de données, pas juste du câblage d'interface).

**Problème rencontré** : les recherches par clé (`get`/`set`/`remove`/`has`) tenaient le verrou (`Mutex`) du tableau de clés pendant la comparaison d'égalité. Appeler `valueEquals` (du bytecode utilisateur, potentiellement ré-entrant) pendant que ce verrou est tenu risquait un deadlock si le code utilisateur touchait la même map.
**Solution** : les clés sont copiées (`clone()` du `Vec<Value>`, le verrou est relâché) avant toute comparaison d'égalité — même précaution que celle déjà documentée ailleurs dans `nl-vm` pour `SET_FIELD` (ne jamais tenir un verrou pendant un rappel dans du code utilisateur).

### `Stringable.toString()` dans la concaténation `+` et le cast `(string)`
`nl-sema` accepte maintenant un opérande de concaténation/cast dont le type statique est une classe implémentant `Stringable` (directement ou via héritage). Côté VM, l'opcode `TO_STRING` (partagé par `+`, `(string)`, et la normalisation `system.Out.print`/`println`) appelle `toString()` par dispatch virtuel quand l'objet implémente `Stringable`, au lieu de toujours utiliser la représentation `[object ClassName]`.

**Problème rencontré** : aucun — en creusant `nl-codegen`, l'émission de bytecode pour `+`/`(string)` était déjà entièrement générique (elle émet `TO_STRING` pour *tout* opérande qui n'est pas déjà une `string`, sans jamais distinguer les primitives des objets). Tout le déficit était donc concentré dans (a) la vérification de type `nl-sema`, qui rejetait catégoriquement tout type `Named`, et (b) l'opcode `TO_STRING` côté VM, qui ne savait produire que la représentation par défaut. Aucun changement de `nl-codegen` n'a été nécessaire — plus simple que prévu initialement.

---

## 2. Syntaxe / sémantique absente

- **`typedef`** — absent. Mot-clé lexé mais jamais parsé en déclaration ni utilisé comme alias. C'est l'unique tâche listée dans `Next.md`.
- **`switch`/`case`** — absent du parseur (`crates/nl-syntax/src/parser.rs` ne traite jamais `Keyword::Switch`). Seule l'expression `match(...)` existe. Confirmé par test manuel (`parse error ... expected expression, found Keyword(Switch)`).
- **`interface A extends B, C`** (héritage d'interfaces) — non parsé. `parse_interface_decl` va du nom directement à `{`. Corollaire runtime : `implements_interface` ne remonte pas transitivement les interfaces étendues.
- **`for (const auto item : collection)` explicite** — le `const` est consommé puis jeté ; `StmtKind::ForEach` n'a pas de champ `is_const`. Seuls les cas *implicites* (méthode `const`, paramètre `const`/`const ref`) sont vérifiés (E039) ; la forme explicite documentée n'est jamais appliquée.
- **`E030`** (identifiant = mot-clé réservé) — code absent de `crates/nl-sema/src/error.rs`. Seul diagnostic de la liste E001-E049/W001 manquant.
- **Validation de type pour `operator++`/`operator--`** — aucune vérification sémantique (pas d'E009) ; l'erreur ne surgit qu'au codegen sous forme d'erreur non structurée.
- **Résolution de surcharge par arité uniquement** (limitation de longue date, notée dans `journal/`) — sauf les opérateurs, qui matchent par type exact.
- **`++`/`--`** : forme postfixe uniquement, pas de valeur d'expression réelle.

## 3. VM / bytecode

- **Drapeaux `ABSTRACT`/`FINAL`** définis dans `crates/nl-bytecode/src/module.rs` mais jamais émis par `nl-codegen`, ni vérifiés par `nl-vm`. Les méthodes abstraites sont simplement omises du bytecode au lieu d'être codées `code_length = 0` comme le prescrit vm.md. La protection existe côté `nlc` (E032/E035/E036 au compile-time) mais pas comme filet de sécurité VM pour un `.nlm` généré autrement.
- **Garbage collector** = comptage de références (`Arc`), documenté et assumé dans le code, mais sans collecteur de cycles : un cycle d'objets n'est jamais réclamé et ses destructeurs ne s'exécutent jamais.
- **Dispatch virtuel** = recherche linéaire par nom+descripteur en remontant `extends`, pas de vtable précalculée comme le décrit vm.md § Method dispatch (comportement correct, juste pas l'implémentation documentée — impact perf non mesuré).
- **Traces de pile** : granularité par *statement* seulement (une closure à corps-expression n'a aucune entrée de ligne) ; pas de nom de méthode dans `ExecutionPoint`.
- **Target-typing des closures** (specs.md règle #5) non implémenté — pas de widening automatique.
- **Capture `auto` dans les closures** : seules les variables explicitement typées sont boxées ; une capture `auto` mutée reste par valeur.

## 4. Bibliothèque standard (stdlib.md)

### Namespaces entièrement absents
- **`system.text.json`** — `JsonValue` et toute la famille, `Json.parse/tryParse/stringify`, `JsonFormatException`. Aucune trace dans le code.
- **`system.db`** (+ `system.db.sqlite`, `system.db.mysql`) — `Connection`, `PreparedStatement`, `ResultSet`, `Row`, `ColumnType`, `SqlException`, drivers Sqlite/Mysql. Aucune dépendance driver dans les `Cargo.toml`.

### Écarts documentés dans le code
- **`system.io.File.glob`/`system.io.Grep`** — `mini_regex.rs` ne compile que des regex ; un pattern glob littéral (`"*.txt"`) ne matche pas comme un glob (`*` y est un quantificateur regex). Comportement volontaire et commenté, mais en décalage avec stdlib.md qui présente glob et regex comme équivalents.
- **`system.thread.Thread.join()/sleep()`** — déclarent `throws InterruptedException` pour le typage, mais aucun mécanisme d'interruption réel n'existe ; l'exception ne peut jamais être levée.
- **`system.ps.Process.list()`** — lit `/proc` directement, Linux uniquement.
- **`system.In.readLine`** — fonctionne mais n'est exercé par aucun test.

## 5. Optimisations (milestone 9 — optionnel, non commencé)

Aucune des optimisations listées dans `optimizations.md` n'est implémentée (constant folding/propagation, dead code elimination, devirtualization, inlining, tail call optimization, string literal concatenation, incremental compilation côté compilateur ; string interning, JIT, superinstructions, inline caching, GC tuning côté VM). Attendu : ce milestone est explicitement marqué optionnel et non commencé dans `journal/journal_01_initial_build.md`.

## 6. Divers mineurs (outillage, pas la spec elle-même)

- `crates/nl-test-runner/src/main.rs:10` a un chemin par défaut codé en dur vers `/data/projects/nlvm-specs/tests`, obsolète depuis la migration vers `nlvm-lang/nlvm-specs` (il faut le passer explicitement en argument).
- Aucune vérification de conformance d'interface au-delà d'E044 (const-correctness nom+arité) : les types de retour/paramètres d'une méthode d'interface ne sont pas comparés à son implémentation.
- `Self` en interface non testé pour l'appel via une variable de type interface (`Cloneable c = new Point(); c.clone()`).

---

## Étape suivante recommandée

Les trois écarts à fort impact étant traités et couverts par des tests (§ Phase 1), l'ordre de valeur/risque décroissant pour la suite :
1. `typedef` — déjà la tâche unique de `Next.md`, donc alignée avec le plan existant.
2. `switch`/`case` — sucre syntaxique, `match` couvre déjà le besoin fonctionnel, donc moins urgent malgré sa visibilité dans les specs.
3. `system.db` et `system.text.json` — gros chantiers autonomes (nouvelles dépendances, nouveau binding natif complet) ; à traiter comme des mini-projets séparés une fois les fondations ci-dessus solidifiées.
4. Milestone 9 (optimisations) — à laisser pour la fin, conformément au plan des milestones lui-même.

Écart additionnel découvert en marge de la Phase 1 (voir son propre paragraphe "Problème rencontré" ci-dessus) : `system.Out.print`/`println` continue de rejeter tout argument objet, y compris une classe `Stringable` — specs.md § Stringable interface le liste pourtant comme troisième consommateur (avec `+` et `(string)`). L'opcode VM sous-jacent (`TO_STRING`) est déjà corrigé ; seule la vérification côté `nl-codegen::compile_stdlib_call` (`is_printlike`) reste à assouplir. Non traité ici pour rester strictement dans le périmètre des trois écarts demandés.
