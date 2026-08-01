//! Optimization pipeline — optimizations.md § Compiler optimizations.
//!
//! Every optimization the spec lists for the compiler is **optional**
//! (`may`), and principle 3 (*implementation freedom*) says correctness must
//! never depend on one being applied. This module is where that stays true
//! in practice: passes are registered in one table with the minimum
//! `OptLevel` they need, `run` applies exactly those the requested level
//! allows, and `-O0` therefore runs nothing at all — not as a debug escape
//! hatch, but as a configuration `nltest --differential` checks against on
//! every fixture (see issue #24).
//!
//! Passes here rewrite already-emitted modules. An optimization that has to
//! happen while the AST is still around (constant folding, dead code
//! elimination) can't be one of these — it belongs in `expr`/`stmt`, gated
//! on the same `OptLevel`, which `compile_program_with` threads through.

use nl_bytecode::{Module, OptLevel};

/// A pass: its name (for `nlc -v`), the lowest level that enables it, and
/// the rewrite itself. A plain function table rather than a trait — passes
/// need no state, and this keeps the whole registry readable at a glance.
type Pass = (&'static str, OptLevel, fn(&mut Module));

/// The registry. Empty on purpose: the flag and the differential harness
/// land before the passes they exist to guard, so nothing can be added under
/// issue #18 without a way to prove it optional. Adding a pass is one line
/// here plus its function.
const PASSES: &[Pass] = &[];

/// Names of the passes `run` would apply at `level`, in registry order —
/// `nlc --verbose` prints this rather than re-deriving the list, so it can
/// never claim a pass that didn't run. Empty at `-O0`, by definition.
pub fn enabled_passes(level: OptLevel) -> Vec<&'static str> {
    PASSES
        .iter()
        .filter(|(_, min_level, _)| level >= *min_level)
        .map(|(name, _, _)| *name)
        .collect()
}

/// Runs every pass enabled at `level` over `modules`, in registry order.
pub fn run(modules: &mut [Module], level: OptLevel) {
    for (_, min_level, pass) in PASSES {
        if level < *min_level {
            continue;
        }
        for module in modules.iter_mut() {
            pass(module);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn o0_runs_no_pass() {
        assert!(enabled_passes(OptLevel::O0).is_empty());
    }

    /// Every pass is registered at a level ≥ O1, so `-O0` stays the
    /// "nothing ran" configuration however the table grows.
    #[test]
    fn no_pass_is_registered_below_o1() {
        assert!(PASSES.iter().all(|(_, level, _)| *level >= OptLevel::O1));
    }

    /// Guards the ordering invariant the registry relies on: a pass is
    /// enabled when the requested level is *at least* its minimum, so the
    /// levels must compare in the order they are declared.
    #[test]
    fn levels_are_ordered() {
        assert!(OptLevel::O0 < OptLevel::O1);
        assert_eq!(OptLevel::default(), OptLevel::O0);
    }
}
