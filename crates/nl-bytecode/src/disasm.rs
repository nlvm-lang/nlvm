//! Linear-sweep decoding of a method's code array into instructions.
//!
//! Enough to *walk* bytecode without executing it — which is what a
//! link-time verifier needs (`nl_vm::program::verify_link` scans every
//! `NEW` target for the `ABSTRACT` flag, vm.md § Class flag bits) and what
//! any future disassembler/dumping tool would build on.
//!
//! A linear sweep is exact here, not a heuristic: this encoding has no
//! variable-length instruction, no operand alignment padding and no inline
//! data (jump tables, constants) embedded in the code array, so the first
//! byte of a method is an opcode and every subsequent instruction starts
//! right after the previous one's operands (`Opcode::operand_len`). That
//! is what makes it safe to conclude "this method contains no `NEW` of an
//! abstract class" rather than merely "no such `NEW` was spotted".

use crate::error::BytecodeError;
use crate::opcode::Opcode;

/// One decoded instruction: its position in the code array, its opcode, and
/// a borrowed slice of its operand bytes (empty for operand-less opcodes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Instruction<'a> {
    pub pc: usize,
    pub opcode: Opcode,
    pub operands: &'a [u8],
}

impl Instruction<'_> {
    /// The big-endian `u16` starting at `offset` bytes into the operands —
    /// how constant-pool indices are encoded (`NEW`'s class index,
    /// `INVOKE_*`'s method-ref index, ...). `None` if this instruction's
    /// operands don't reach that far.
    pub fn operand_u16(&self, offset: usize) -> Option<u16> {
        let bytes = self.operands.get(offset..offset + 2)?;
        Some(u16::from_be_bytes([bytes[0], bytes[1]]))
    }
}

/// Iterator over `code`'s instructions, in address order.
///
/// Yields `Err` once — then stops — if the sweep hits a byte that isn't a
/// known opcode, or an instruction whose operands run past the end of the
/// code array. Both mean the code array is not decodable at all, so a
/// caller verifying a property over every instruction must treat the error
/// as "cannot verify" (i.e. reject), never as "end of code".
pub struct Instructions<'a> {
    code: &'a [u8],
    pc: usize,
    done: bool,
}

/// Walks `code` instruction by instruction — see `Instructions`.
pub fn instructions(code: &[u8]) -> Instructions<'_> {
    Instructions {
        code,
        pc: 0,
        done: false,
    }
}

impl<'a> Iterator for Instructions<'a> {
    type Item = Result<Instruction<'a>, BytecodeError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done || self.pc >= self.code.len() {
            return None;
        }
        let pc = self.pc;
        let Some(opcode) = Opcode::from_u8(self.code[pc]) else {
            self.done = true;
            return Some(Err(BytecodeError::Malformed(
                "unknown opcode byte in code array",
            )));
        };
        let start = pc + 1;
        let end = start + opcode.operand_len();
        let Some(operands) = self.code.get(start..end) else {
            self.done = true;
            return Some(Err(BytecodeError::Malformed(
                "instruction operands run past the end of the code array",
            )));
        };
        self.pc = end;
        Some(Ok(Instruction {
            pc,
            opcode,
            operands,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn walks_mixed_operand_widths() {
        // NOP | BI_PUSH 7 | NEW #258 | IINC 1, -1 | RETURN
        let code = [
            Opcode::Nop as u8,
            Opcode::BiPush as u8,
            7,
            Opcode::New as u8,
            1,
            2,
            Opcode::IInc as u8,
            0,
            1,
            0xff,
            0xff,
            Opcode::Return as u8,
        ];
        let decoded: Vec<_> = instructions(&code)
            .map(|i| i.expect("well-formed code"))
            .collect();
        let shape: Vec<_> = decoded.iter().map(|i| (i.pc, i.opcode)).collect();
        assert_eq!(
            shape,
            vec![
                (0, Opcode::Nop),
                (1, Opcode::BiPush),
                (3, Opcode::New),
                (6, Opcode::IInc),
                (11, Opcode::Return),
            ]
        );
        assert_eq!(decoded[2].operand_u16(0), Some(258));
        assert_eq!(decoded[3].operand_u16(2), Some(0xffff));
        assert_eq!(decoded[4].operand_u16(0), None);
    }

    #[test]
    fn reports_unknown_opcode() {
        let err = instructions(&[200]).next().expect("one item");
        assert!(matches!(err, Err(BytecodeError::Malformed(_))));
    }

    #[test]
    fn reports_truncated_operands() {
        // NEW with only one of its two index bytes present.
        let code = [Opcode::New as u8, 0];
        let items: Vec<_> = instructions(&code).collect();
        assert_eq!(items.len(), 1);
        assert!(matches!(items[0], Err(BytecodeError::Malformed(_))));
    }
}
