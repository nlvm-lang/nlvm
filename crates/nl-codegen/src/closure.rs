//! Thin re-export of the shared closure capture/mutation analysis —
//! `Emitter::compile_closure` and friends just need the name sets; the
//! actual AST walk lives in `nl_syntax::capture` so `nl-sema` can reuse the
//! exact same "captured ∩ mutated" analysis (see issue #15) without
//! `nl-codegen` and `nl-sema` depending on each other.
pub(crate) use nl_syntax::capture::{boxed_captures, boxed_captures_in_block, referenced_names};
