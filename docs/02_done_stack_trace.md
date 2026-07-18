# Chantier : stack trace info

Objectif : satisfaire compiler.md/vm.md/specs.md (Milestone 6 — Exceptions & closures) sur `Exception.stackTrace` et `StackOverflowException`. Actuellement rien n'est implémenté (cf. `crates/nl-syntax/src/prelude.rs:9-13`).

Références specs (nlvm-specs) :
* `docs/specs.md:2429-2439` — `Exception.stackTrace: ExecutionPoint[]`, `ExecutionPoint { line, file }`
* `docs/specs.md:2486-2490`, `docs/vm.md:696-705` — capture native par la VM pendant `super(...)` du constructeur `Exception` (pas de bypass `readonly`), frames du chaînage de constructeurs de l'exception elle-même exclues
* `docs/vm.md:286-297` — line-number table par méthode (`{start_pc, line}`), lignes à `0` si absente
* `docs/vm.md:311-324`, `docs/specs.md:2466` — dépassement de profondeur d'appel → `StackOverflowException`
* `docs/vm.md:688,1042,1073` — exception non attrapée sortant de `main` : message + trace sur stderr, exit code 1
* `review/security-audit.md:352-362` (SEC-17) — ne pas exposer les traces en prod (fuite de chemins internes) ; le design "capture dans le constructeur" est un choix de sécurité, à ne pas contourner

## Étapes

### 1. Peupler `line_table` en codegen — ~~FAIT~~
- [x] `crates/nl-codegen/src/lib.rs:305` — remplacer `line_table: Vec::new()` par la vraie table construite pendant l'émission des instructions
- [x] `crates/nl-codegen/src/expr.rs:756` — idem (méthode `invoke` synthétique des closures)
- [x] Vérifier le round-trip lecture/écriture déjà supporté par `crates/nl-bytecode/src/module.rs:71,178-179,296-313` (`LineTableEntry{start_pc,line}`)
- [x] Test : `crates/nl-codegen/src/lib.rs` (`mod tests::line_table_tracks_source_lines`) compile un petit programme et vérifie `start_pc` strictement croissant + lignes exactes

Implémentation : `Emitter` (`crates/nl-codegen/src/expr.rs`) porte désormais `line_table: Vec<LineTableEntry>` + `last_line: u32`, peuplés par `record_line(line)` appelée en tête de `compile_stmt` (`crates/nl-codegen/src/stmt.rs`). Granularité **statement** (l'AST ne porte de `line` qu'au niveau `Stmt`, pas `Expr` — donc un closure à corps expression `() => 42` n'a toujours aucune entrée, cf. limitation notée dans le code). Dédoublonnage sur changement de ligne uniquement, conforme à vm.md § Method descriptor ("entries sorted by ascending start_pc, each covering up to the next entry's start_pc"). 130 tests maison + 14/14 nlvm-specs toujours verts, aucune régression fmt/clippy (les warnings existants dans `expr.rs:1567+` sont préexistants, non liés à ce changement).

### 2. Pile de frames NL walkable dans l'interpréteur — ~~FAIT~~
- [x] Stratégie retenue : pile parallèle légère (`thread_local!`), pas une vraie VM-stack — suffit pour le tracing, et donne l'isolation par thread OS réel (`native::construct_thread`) gratuitement
- [x] Nouveau module `crates/nl-vm/src/call_stack.rs` : `push_frame(class_fqcn, method_name) -> FrameGuard` (RAII, pop au drop — couvre tous les chemins de sortie de `run_frame`, y compris `?`/erreur), `set_current_line(line_table, pc)` (résout la ligne courante via la `line_table` de l'étape 1, `partition_point`), `snapshot(skip)` (liste `(class_fqcn, method_name, line)` innermost-first, `skip` pour exclure les frames du chaînage de constructeurs d'exception — pas encore appelé, prévu étape 4)
- [x] Intégré dans `crates/nl-vm/src/interpreter.rs::run_frame` : guard poussé une fois en tête, `set_current_line` appelé à chaque itération de la boucle avant `exec_step`
- [x] Tests unitaires dans `call_stack.rs` (résolution ligne, push/pop/imbrication, `skip`)

Limitation assumée : `snapshot`/`skip` sont actuellement du code mort (`#[allow(dead_code)]`) — rien ne les appelle encore, ce sera fait à l'étape 4. 130 tests maison + 14/14 nlvm-specs toujours verts.

### 3. Garde de profondeur d'appel → `StackOverflowException` — ~~FAIT~~
- [x] `call_stack::push_frame` (crates/nl-vm/src/call_stack.rs) refuse de pousser au-delà de `MAX_CALL_DEPTH` (renvoie `Result<FrameGuard, ()>`) ; `interpreter::run_frame` convertit l'échec en `throw_native("StackOverflowException", ...)` — l'exception est levée comme si c'était l'instruction `INVOKE_*` *appelante* qui l'avait levée (le nouveau frame n'est jamais poussé), donc elle remonte normalement à travers la table d'exceptions de l'appelant et reste attrapable par un `try`/`catch` NL
- [x] `StackOverflowException` était déjà déclarée dans la hiérarchie prelude (`crates/nl-syntax/src/prelude.rs:37`) mais jamais levée — c'est maintenant fait
- [x] Seuil déterminé **empiriquement** (pas arbitraire) : bisection sur ce poste, build debug, sur un programme `recurse(n) { return recurse(n-1)+1; }` (plus coûteux en pile qu'une récursion terminale) — crash natif Rust vers 300-350 frames aussi bien sur le thread principal (pile 8 MiB, `ulimit -s`) que sur un `system.thread.Thread` spawné. `MAX_CALL_DEPTH = 150` garde une marge ~2x.
- [x] Effet de bord nécessaire : `native::dispatch_thread`'s `std::thread::spawn` (démarrage de `system.thread.Thread`) donnait par défaut une pile ~2 MiB, bien plus petite que celle du thread principal — un seuil sûr sur le thread principal aurait donc quand même crashé nativement sur un thread spawné. Remplacé par `std::thread::Builder::new().stack_size(8 MiB).spawn(...)` pour aligner les deux, afin qu'un seul et même `MAX_CALL_DEPTH` reste sûr partout.
- [x] Test unitaire `call_stack::tests::push_frame_rejects_past_max_depth`
- [x] Test end-to-end `tests/phase9_0040_stack_overflow_exception.yaml` (récursion infinie → catch NL, exit code 7) + validation manuelle supplémentaire (thread principal non catché, thread spawné catché dans la closure, récursion légitime profondeur 120 toujours OK) — nettoyée après coup, pas laissée dans le repo au-delà de la fixture yaml

Limitation assumée : le seuil (150) est calibré empiriquement sur cette machine en build debug ; pas de garantie formelle multi-plateforme, mais marge ~2x prise volontairement. 131 tests maison (130 + la nouvelle fixture) + 14/14 nlvm-specs toujours verts, aucune régression clippy/fmt.

### 4. Champ `stackTrace` + capture native au constructeur `Exception` — ~~FAIT~~
- [x] `ExecutionPoint { file: string, line: int }` déclarée comme vraie classe prelude (`crates/nl-syntax/src/prelude.rs::execution_point_class`) — jamais construite via son propre constructeur NL, uniquement déclarée pour que `Exception.stackTrace: ExecutionPoint[]` soit un type résoluble par nl-sema/nl-codegen comme n'importe quel tableau d'objets
- [x] `Exception` (classe racine, `parent.is_none()`) porte désormais le champ `stackTrace` en plus de `message`
- [x] Capture native dans `crates/nl-vm/src/interpreter.rs` : `maybe_capture_stack_trace` appelée juste avant le retour d'un frame dont la classe est littéralement `"Exception"` et qui est un constructeur (`is_exception_root_ctor`) — écrit directement dans `locals[0].fields`, en bypassant `SET_FIELD`/bytecode (cohérent avec vm.md : la readonly-rule l'aurait de toute façon autorisé puisqu'on est dans le constructeur déclarant, donc aucun bypass n'était nécessaire côté spec, seulement plus simple à implémenter ainsi)
- [x] Exclusion des frames du chaînage de constructeurs de l'exception : `exception_constructor_chain_depth` remonte `extends` depuis la classe **runtime** de `this` (pas la classe statique "Exception" du frame courant) jusqu'à `Exception` inclus — le nombre de classes dans cette chaîne est exactement le nombre de frames encore vivantes au sommet de la pile à ce moment précis (chaque ctor parent est en pause au milieu de son `super(...)`), validé empiriquement par `phase5_0090` (trace démarre bien à la ligne du `new`, pas dans le constructeur)
- [x] `throw_native` (exceptions levées nativement par la VM : division par zéro, null deref, out-of-bounds, `StackOverflowException`...) peuple aussi `stackTrace`, avec `skip=0` (pas de chaîne de constructeur à exclure — le throw natif se produit directement dans le frame fautif)
- [x] `file` dérivé du FQCN de la classe déclarante de chaque frame (`namespace.Class` → `namespace/Class.nl`, ex. `phase9.stacktracefields.Main` → `phase9/stacktracefields/Main.nl`) — vm.md dit juste "derived from the module's class name and namespace", pas de format canonique imposé ; les classes de closures synthétiques (`Main$m0$closure0`) produisent un nom un peu étrange (`Main$m0$closure0.nl`) mais restent conformes à la lettre du spec
- [x] `crates/nl-vm/src/error.rs` : pas touché — `VmError::Thrown(Value)` porte déjà l'objet exception complet (avec son `stackTrace` maintenant peuplé), rien à ajouter côté enum
- [x] Sortie sur exception non attrapée (`crates/nl-vm/src/program.rs::describe_exception`, utilisée par `run_program` et par `native::dispatch_thread` pour les threads) : ajoute une ligne `\tat file:line` par frame après `ClassName: message` — format libre côté spec ("implementation-defined")
- [x] Effet de bord découvert en testant : `system.thread.Thread` (`native.rs`) donnait par défaut une pile ~2 MiB (étape 3) — déjà corrigé à l'étape 3, revalidé ici via `phase6_0210` dont la trace pointe correctement dans la closure du thread

Limitation assumée : `ExecutionPoint` n'a que `{file, line}` (pas de nom de méthode) — conforme à `specs.md:2436-2439`, mais rend les lignes stderr moins riches qu'un trace Java typique.

### 5. Tests — ~~FAIT~~
- [x] `tests/phase9_0050_exception_stack_trace_fields.yaml` — deux fichiers séparés (`Helper.nl`/`Main.nl`), vérifie `stackTrace.length()==2` et le contenu exact `file:line` des deux frames (throw site + call site), preuve que le walk multi-frame fonctionne, pas juste le frame courant
- [x] 3 fixtures existantes mises à jour pour le nouveau stderr enrichi (comportement correct, pas une régression) : `tests/phase4_0060_array_out_of_bounds.yaml`, `tests/phase5_0090_uncaught_custom_exception.yaml` (valide la profondeur de chaîne de constructeur : `MyException extends Exception`, trace démarre bien au `throw new MyException(...)`, pas dans un constructeur), `tests/phase6_0210_thread_uncaught_exception.yaml`
- [x] 132 tests maison (130 + `phase9_0040` étape 3 + `phase9_0050` étape 4) + 14/14 nlvm-specs, aucune régression fmt/clippy sur les fichiers touchés

**Chantier terminé.** Les 4 étapes fonctionnelles + tests sont faites ; `Next.md` peut être mis à jour pour marquer "stack trace info" comme fait.

## Notes
- Ne pas faire de shim provisoire (pas de trace "vide mais présente à l'API") — soit c'est fait proprement (lignes réelles), soit ça reste absent du prelude comme aujourd'hui.
- Chantier à traiter dans l'ordre 1 → 2 → 3/4 en parallèle → 5, chaque étape dépendant de la précédente pour être testable de bout en bout.
