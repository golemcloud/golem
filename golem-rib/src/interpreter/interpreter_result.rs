use crate::interpreter::interpreter_stack_value::RibInterpreterStackValue;
use crate::{GetLiteralValue, LiteralValue};
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;

pub enum RibResult {
    Unit,
    Val(TypeAnnotatedValue),
}

impl RibResult {
    pub fn from_rib_interpreter_stack_value(
        stack_value: &RibInterpreterStackValue,
    ) -> Option<RibResult> {
        match stack_value {
            RibInterpreterStackValue::Unit => Some(RibResult::Unit),
            RibInterpreterStackValue::Val(type_annotated_value) => {
                Some(RibResult::Val(type_annotated_value.clone()))
            }
            RibInterpreterStackValue::Iterator(_) => None,
            RibInterpreterStackValue::Sink(_, _) => None,
        }
    }

    pub fn get_bool(&self) -> Option<bool> {
        match self {
            RibResult::Val(TypeAnnotatedValue::Bool(bool)) => Some(*bool),
            RibResult::Val(_) => None,
            RibResult::Unit => None,
        }
    }
    pub fn get_val(&self) -> Option<TypeAnnotatedValue> {
        match self {
            RibResult::Val(val) => Some(val.clone()),
            RibResult::Unit => None,
        }
    }

    pub fn get_literal(&self) -> Option<LiteralValue> {
        self.get_val().and_then(|x| x.get_literal())
    }
}
