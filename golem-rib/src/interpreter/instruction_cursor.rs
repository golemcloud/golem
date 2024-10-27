use crate::{InstructionId, RibByteCode, RibIR};

pub struct RibByteCodeCursor {
    byte_code: RibByteCode,
    position: usize,
}

impl RibByteCodeCursor {
    pub fn from_rib_byte_code(byte_code: RibByteCode) -> RibByteCodeCursor {
        RibByteCodeCursor {
            byte_code,
            position: 0,
        }
    }

    pub fn get_instruction(&mut self) -> Option<RibIR> {
        if self.position < self.byte_code.instructions.len() {
            let ir = self.byte_code.instructions[self.position].clone();
            self.position += 1;
            Some(ir)
        } else {
            None
        }
    }

    pub fn move_to(&mut self, move_to: &InstructionId) -> Option<()> {
        for (index, current_instruction) in self.byte_code.instructions.iter().enumerate() {
            if let Some(label_id) = current_instruction.get_instruction_id() {
                if label_id.index == move_to.index {
                    self.position = index + 1;
                    return Some(());
                }
            }
        }

        None
    }
}
