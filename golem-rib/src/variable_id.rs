use bincode::{Decode, Encode};

#[derive(Debug, Clone, PartialEq, Encode, Decode)]
pub struct VariableId(Option<u16>);

impl VariableId {
    pub fn is_none(&self) -> bool {
        self.0.is_none()
    }

    pub fn init() -> Self {
        VariableId(None)
    }
    pub fn increment(&mut self) -> VariableId {
        let new_variable_id = self.0.map_or(Some(0), |x| Some(x + 1));
        self.0 = new_variable_id;
        VariableId(new_variable_id)
    }
}
