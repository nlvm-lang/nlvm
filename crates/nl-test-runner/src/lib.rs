//! The fixture format (`---` YAML front matter + `#NLFILE`-separated source
//! blocks) shared by the two harnesses that read multi-file NL programs from a
//! single file: `nltest` (this crate's binary, over `tests/`) and `nlbench`
//! (`nl-bench`, over `benches/`). Only the *format* is shared — each harness
//! deserializes its own header type from the front matter.

pub mod fixture;
