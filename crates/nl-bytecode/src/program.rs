//! Program container format (`.nlp`) — a single file bundling every module
//! of a compiled program, so `nlvm` can run one file instead of a directory
//! of `.nlm` modules.
//!
//! Layout (big-endian, like the module format):
//!
//! ```text
//! u32 magic ("NLP\0") | u16 version | u16 module_count
//! then module_count times: u32 byte_len | module bytes
//! ```
//!
//! Each embedded module is a complete `.nlm` image (own magic, version and
//! integrity trailer) decoded with `Module::decode` — the container adds no
//! per-module metadata of its own, so the module format can evolve without
//! touching the container.

use crate::error::BytecodeError;
use crate::module::{Module, Reader};

pub const PROGRAM_MAGIC: u32 = 0x4E4C_5000;
pub const PROGRAM_VERSION: u16 = 1;

/// Whether `bytes` starts with the `.nlp` container magic — used by loaders
/// to tell a program container apart from a bare `.nlm` module image.
pub fn is_program(bytes: &[u8]) -> bool {
    bytes.len() >= 4 && bytes[..4] == PROGRAM_MAGIC.to_be_bytes()
}

pub fn encode_program(modules: &[Module]) -> Vec<u8> {
    let mut buf = Vec::new();
    buf.extend_from_slice(&PROGRAM_MAGIC.to_be_bytes());
    buf.extend_from_slice(&PROGRAM_VERSION.to_be_bytes());
    buf.extend_from_slice(&(modules.len() as u16).to_be_bytes());
    for module in modules {
        let bytes = module.encode();
        buf.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
        buf.extend_from_slice(&bytes);
    }
    buf
}

/// Modules keep their container order — callers pass them to the VM exactly
/// as a multi-`.nlm` invocation would.
pub fn decode_program(bytes: &[u8]) -> Result<Vec<Module>, BytecodeError> {
    let mut r = Reader::new(bytes);

    let magic = r.read_u32()?;
    if magic != PROGRAM_MAGIC {
        return Err(BytecodeError::BadMagic(magic));
    }
    let _version = r.read_u16()?;

    let module_count = r.read_u16()?;
    let mut modules = Vec::with_capacity(module_count as usize);
    for _ in 0..module_count {
        let len = r.read_u32()? as usize;
        let module_bytes = r.read_bytes(len)?;
        modules.push(Module::decode(module_bytes)?);
    }
    Ok(modules)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constant_pool::ConstantPool;
    use crate::module::{HashAlgo, MAGIC, VERSION};

    fn test_module(class_name: &str) -> Module {
        let mut constant_pool = ConstantPool::new();
        let this_class = constant_pool.add_class(class_name);
        Module {
            version: VERSION,
            constant_pool,
            this_class,
            class_flags: 0,
            super_class: 0,
            interfaces: Vec::new(),
            fields: Vec::new(),
            methods: Vec::new(),
            hash_algo: HashAlgo::Sha256,
        }
    }

    #[test]
    fn roundtrip_preserves_modules_and_order() {
        let modules = vec![test_module("app.Main"), test_module("app.Helper")];
        let bytes = encode_program(&modules);

        assert!(is_program(&bytes));
        let decoded = decode_program(&bytes).unwrap();
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].this_class_name(), Some("app.Main"));
        assert_eq!(decoded[1].this_class_name(), Some("app.Helper"));
    }

    #[test]
    fn empty_program_roundtrips() {
        let bytes = encode_program(&[]);
        assert!(decode_program(&bytes).unwrap().is_empty());
    }

    #[test]
    fn module_image_is_not_a_program() {
        let bytes = test_module("app.Main").encode();
        assert!(!is_program(&bytes));
        assert!(matches!(
            decode_program(&bytes),
            Err(BytecodeError::BadMagic(m)) if m == MAGIC
        ));
    }

    #[test]
    fn truncated_container_fails() {
        let bytes = encode_program(&[test_module("app.Main")]);
        assert!(matches!(
            decode_program(&bytes[..bytes.len() - 1]),
            Err(BytecodeError::UnexpectedEof)
        ));
    }
}
