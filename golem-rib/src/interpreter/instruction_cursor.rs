// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

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
