pub mod constant_pool;
pub mod disasm;
pub mod error;
pub mod module;
pub mod opcode;
pub mod program;

pub use constant_pool::{ConstantPool, ConstantPoolEntry};
pub use disasm::{instructions, Instruction};
pub use error::BytecodeError;
pub use module::{
    class_flags, field_flags, method_flags, ExceptionTableEntry, FieldDescriptor, HashAlgo,
    LineTableEntry, MethodDescriptor, Module, OptLevel,
};
pub use opcode::Opcode;
pub use program::{decode_program, encode_program, is_program, PROGRAM_MAGIC, PROGRAM_VERSION};
