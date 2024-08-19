use crate::interpreter::env::{EnvironmentKey, InterpreterEnv, RibFunctionInvoke};
use crate::interpreter::result::RibInterpreterResult;
use crate::interpreter::stack::InterpreterStack;
use crate::{RibByteCode, RibIR};
use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
use std::collections::{HashMap, VecDeque};

pub struct Interpreter {
    pub stack: InterpreterStack,
    pub env: InterpreterEnv,
}

impl Interpreter {
    pub fn default() -> Self {
        Interpreter {
            stack: InterpreterStack::new(),
            env: InterpreterEnv::default(),
        }
    }

    pub fn new(
        input: HashMap<String, TypeAnnotatedValue>,
        function_invoke: RibFunctionInvoke,
    ) -> Self {
        let input = input
            .into_iter()
            .map(|(k, v)| (EnvironmentKey::from_global(k), RibInterpreterResult::Val(v)))
            .collect();

        Interpreter {
            stack: InterpreterStack::new(),
            env: InterpreterEnv::new(input, function_invoke),
        }
    }

    pub fn from_input(env: HashMap<String, TypeAnnotatedValue>) -> Self {
        Interpreter {
            stack: InterpreterStack::new(),
            env: InterpreterEnv::from_input(env),
        }
    }

    pub async fn run(
        &mut self,
        instructions0: RibByteCode,
    ) -> Result<RibInterpreterResult, String> {
        // O(1) to do this
        let mut instructions = VecDeque::from(instructions0.instructions);

        while let Some(instruction) = instructions.pop_front() {
            match instruction {
                RibIR::PushLit(val) => {
                    self.stack.push_val(val);
                }

                RibIR::PushFlag(val) => {
                    self.stack.push_val(val);
                }

                RibIR::CreateAndPushRecord(analysed_type) => {
                    internal::run_create_record_instruction(analysed_type, &mut self.stack)?;
                }

                RibIR::UpdateRecord(field_name) => {
                    internal::run_update_record_instruction(field_name, &mut self.stack)?;
                }

                RibIR::PushList(analysed_type, arg_size) => {
                    internal::run_push_list_instruction(arg_size, analysed_type, &mut self.stack)?;
                }

                RibIR::EqualTo => {
                    internal::run_compare_instruction(&mut self.stack, |left, right| {
                        left == right
                    })?;
                }

                RibIR::GreaterThan => {
                    internal::run_compare_instruction(&mut self.stack, |left, right| left > right)?;
                }

                RibIR::LessThan => {
                    internal::run_compare_instruction(&mut self.stack, |left, right| left < right)?;
                }

                RibIR::GreaterThanOrEqualTo => {
                    internal::run_compare_instruction(&mut self.stack, |left, right| {
                        left >= right
                    })?;
                }

                RibIR::LessThanOrEqualTo => {
                    internal::run_compare_instruction(&mut self.stack, |left, right| {
                        left <= right
                    })?;
                }

                RibIR::AssignVar(variable_id) => {
                    internal::run_assign_var_instruction(variable_id, self)?;
                }

                RibIR::LoadVar(variable_id) => {
                    internal::run_load_var_instruction(variable_id, self)?;
                }

                RibIR::JumpIfFalse(instruction_id) => {
                    internal::run_jump_if_false_instruction(
                        instruction_id,
                        &mut instructions,
                        &mut self.stack,
                    )?;
                }

                RibIR::SelectField(field_name) => {
                    internal::run_select_field_instruction(field_name, &mut self.stack)?;
                }

                RibIR::SelectIndex(index) => {
                    internal::run_select_index_instruction(&mut self.stack, index)?;
                }

                RibIR::InvokeFunction(parsed_function_name, arity, _) => {
                    internal::run_call_instruction(
                        parsed_function_name,
                        arity,
                        self,
                    )
                    .await?;
                }

                RibIR::PushVariant(variant_name, analysed_type) => {
                    internal::run_variant_construction_instruction(variant_name, analysed_type, self)
                        .await?;
                }

                RibIR::Throw(message) => {
                    return Err(message);
                }

                RibIR::GetTag => {
                    internal::run_get_tag_instruction(&mut self.stack)?;
                }

                RibIR::Deconstruct => {
                    internal::run_deconstruct_instruction(&mut self.stack)?;
                }

                RibIR::Jump(instruction) => {
                    internal::drain_instruction_stack_until_label(instruction, &mut instructions);
                }

                RibIR::PushSome(analysed_type) => {
                    internal::run_create_some_instruction(&mut self.stack, analysed_type)?;
                }
                RibIR::PushNone(analysed_type) => {
                    internal::run_create_none_instruction(&mut self.stack, analysed_type)?;
                }
                RibIR::PushOkResult(analysed_type) => {
                    internal::run_create_ok_instruction(&mut self.stack, analysed_type)?;
                }
                RibIR::PushErrResult(analysed_type) => {
                    internal::run_create_err_instruction(&mut self.stack, analysed_type)?;
                }
                RibIR::Concat(arg_size) => {
                    internal::run_concat_instruction(&mut self.stack, arg_size)?;
                }
                RibIR::PushTuple(analysed_type, arg_size) => {
                    internal::run_push_tuple_instruction(arg_size, analysed_type, &mut self.stack)?;
                }
                RibIR::Negate => {
                    internal::run_negate_instruction(&mut self.stack)?;
                }

                RibIR::Label(_) => {}
            }
        }

        self.stack
            .pop()
            .ok_or("Empty stack after running the instructions".to_string())
    }
}

mod internal {
    use crate::interpreter::env::EnvironmentKey;
    use crate::interpreter::literal::LiteralValue;
    use crate::interpreter::result::RibInterpreterResult;
    use crate::interpreter::stack::InterpreterStack;
    use crate::{GetLiteralValue, InstructionId, Interpreter, ParsedFunctionName, RibIR, VariableId};
    use golem_wasm_ast::analysis::AnalysedType;
    use golem_wasm_ast::analysis::TypeResult;
    use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
    use golem_wasm_rpc::protobuf::{NameValuePair, TypedRecord, TypedTuple};
    use std::collections::VecDeque;
    use std::ops::Deref;
    use golem_wasm_rpc::protobuf::typed_result::ResultValue;

    pub(crate) fn run_assign_var_instruction(
        variable_id: VariableId,
        interpreter: &mut Interpreter,
    ) -> Result<(), String> {
        let value = interpreter
            .stack
            .pop()
            .ok_or("Expected a value on the stack before assigning a variable".to_string())?;
        let env_key = EnvironmentKey::from(variable_id);

        interpreter.env.insert(env_key, value);
        Ok(())
    }

    pub(crate) fn run_load_var_instruction(
        variable_id: VariableId,
        interpreter: &mut Interpreter,
    ) -> Result<(), String> {
        let env_key = EnvironmentKey::from(variable_id.clone());
        let value = interpreter.env.lookup(&env_key).ok_or(format!(
            "Variable `{}` not found during evaluation of expression",
            variable_id
        ))?;

        interpreter.stack.push(value);
        Ok(())
    }

    pub(crate) fn run_create_record_instruction(
        analysed_type: AnalysedType,
        interpreter_stack: &mut InterpreterStack,
    ) -> Result<(), String> {
        let name_type_pair = match analysed_type {
            AnalysedType::Record(type_record) => type_record
                .fields
                .into_iter()
                .map(|field| golem_wasm_ast::analysis::protobuf::NameTypePair {
                    name: field.name,
                    typ: Some((&field.typ).into()),
                })
                .collect(),
            _ => return Err("Expected a Record type".to_string()),
        };

        interpreter_stack.create_record(name_type_pair);
        Ok(())
    }

    pub(crate) fn run_update_record_instruction(
        field_name: String,
        interpreter_stack: &mut InterpreterStack,
    ) -> Result<(), String> {
        //  The value of field_name
        let last_record = interpreter_stack
            .pop_val()
            .ok_or("Expected a value on the stack".to_string())?;

        let value = interpreter_stack
            .pop_val()
            .ok_or("Expected a record on the stack".to_string())?;

        match last_record {
            TypeAnnotatedValue::Record(record) => {
                let mut existing_fields = record.value;

                dbg!(existing_fields.clone());

                let name_value_pair = NameValuePair {
                    name: field_name.clone(),
                    value: Some(golem_wasm_rpc::protobuf::TypeAnnotatedValue {
                        type_annotated_value: Some(value),
                    }),
                };

                existing_fields.push(name_value_pair);
                interpreter_stack.push_val(TypeAnnotatedValue::Record(TypedRecord {
                    value: existing_fields,
                    typ: record.typ,
                }));

                Ok(())
            }

            _ => Err(format!(
                "Failed to get a record from the stack to set the field {}",
                field_name
            )),
        }
    }

    pub(crate) fn run_push_list_instruction(
        list_size: usize,
        analysed_type: AnalysedType,
        interpreter_stack: &mut InterpreterStack,
    ) -> Result<(), String> {

        // TODO; This type of check is actually un-necessary
        // Avoid these checks - and allow compiler to directly form the instruction with the inner type
         match analysed_type {
            AnalysedType::List(inner_type) => {
                // Last updated value in stack should be a list to update the list
                let last_list = interpreter_stack
                    .pop_n(list_size)
                    .ok_or(format!("Expected {} value on the stack", list_size))?;

                let type_annotated_values = last_list
                    .iter()
                    .map(|interpreter_result| {
                        interpreter_result
                            .get_val()
                            .ok_or("Failed to get value from the stack".to_string())
                    })
                    .collect::<Result<Vec<TypeAnnotatedValue>, String>>()?;

                interpreter_stack.push_list(type_annotated_values, inner_type.inner.deref());

                Ok(())
            }

            _ => Err("Expected a List type".to_string()),
        }
    }

    pub(crate) fn run_push_tuple_instruction(
        list_size: usize,
        analysed_type: AnalysedType,
        interpreter_stack: &mut InterpreterStack,
    ) -> Result<(), String> {

        // TODO; This type of check is actually un-necessary
        // Avoid these checks - and allow compiler to directly form the instruction with the inner type
        match analysed_type {
            AnalysedType::Tuple(inner_type) => {
                // Last updated value in stack should be a list to update the list
                let last_list = interpreter_stack
                    .pop_n(list_size)
                    .ok_or(format!("Expected {} value on the stack", list_size))?;

                let type_annotated_values = last_list
                    .iter()
                    .map(|interpreter_result| {
                        interpreter_result
                            .get_val()
                            .ok_or("Failed to get value from the stack".to_string())
                    })
                    .collect::<Result<Vec<TypeAnnotatedValue>, String>>()?;

                interpreter_stack.push_tuple(type_annotated_values, &inner_type.items);

                Ok(())
            }

            _ => Err("Expected a List type".to_string()),
        }
    }

    pub(crate) fn run_negate_instruction(
        interpreter_stack: &mut InterpreterStack,
    ) -> Result<(), String> {
        let value = interpreter_stack
            .pop_val()
            .ok_or("Failed to get a value from the stack to negate".to_string())?;

        let result = value.get_literal().and_then(|literal| literal.get_bool()).ok_or(
            "Failed to get a boolean value from the stack to negate".to_string(),
        )?;

        interpreter_stack.push_val(TypeAnnotatedValue::Bool(!result));
        Ok(())
    }


    pub(crate) fn run_compare_instruction(
        interpreter_stack: &mut InterpreterStack,
        compare_fn: fn(LiteralValue, LiteralValue) -> bool,
    ) -> Result<(), String> {
        let left = interpreter_stack.pop().ok_or(
            "Empty stack and failed to get a value to do the comparison operation".to_string(),
        )?;
        let right = interpreter_stack.pop().ok_or(
            "Failed to get a value from the stack to do the comparison operation".to_string(),
        )?;

        let result = left.compare(&right, compare_fn)?;

        interpreter_stack.push(result);

        Ok(())
    }

    pub(crate) fn run_select_field_instruction(
        field_name: String,
        interpreter_stack: &mut InterpreterStack,
    ) -> Result<(), String> {
        let record = interpreter_stack
            .pop()
            .ok_or("Failed to get a record from the stack to select a field".to_string())?;

        match record {
            RibInterpreterResult::Val(TypeAnnotatedValue::Record(record)) => {
                let field = record
                    .value
                    .into_iter()
                    .find(|field| field.name == field_name)
                    .ok_or(format!("Field {} not found in the record", field_name))?;

                let value = field.value.ok_or("Field value not found".to_string())?;

                let inner_type_annotated_value = value
                    .type_annotated_value
                    .ok_or("Field value not found".to_string())?;

                interpreter_stack.push_val(inner_type_annotated_value);
                Ok(())
            }
            result => Err(format!(
                "Expected a record value to select a field. Obtained {:?}",
                result
            )),
        }
    }

    pub(crate) fn run_select_index_instruction(
        interpreter_stack: &mut InterpreterStack,
        index: usize,
    ) -> Result<(), String> {
        let record = interpreter_stack
            .pop()
            .ok_or("Failed to get a record from the stack to select a field".to_string())?;

        match record {
            RibInterpreterResult::Val(TypeAnnotatedValue::List(typed_list)) => {
                let value = typed_list
                    .values
                    .get(index)
                    .ok_or(format!("Index {} not found in the list", index))?
                    .clone();

                let inner_type_annotated_value = value
                    .type_annotated_value
                    .ok_or("Field value not found".to_string())?;

                interpreter_stack.push_val(inner_type_annotated_value);
                Ok(())
            }
            result => Err(format!(
                "Expected a sequence value to select an index. But obtained {:?}",
                result
            )),
        }
    }

    pub(crate) async fn run_variant_construction_instruction(
        variant_name: String,
        analysed_type: AnalysedType,
        interpreter: &mut Interpreter,
    ) -> Result<(), String> {
        match analysed_type {
            AnalysedType::Variant(variants) => {
                let variant = variants
                    .cases
                    .iter()
                    .find(|name| &name.name == &variant_name)
                    .ok_or(format!("Unknown variant {} not found", variant_name))?;

                let variant_arg_typ = variant.typ.clone();

                let arg_value = match variant_arg_typ {
                    Some(_) => Some(
                        interpreter
                            .stack
                            .pop_val()
                            .ok_or("Failed to get a value from the stack".to_string())?,
                    ),
                    None => None,
                };

                interpreter.stack.push_variant(
                    variant_name.clone(),
                    arg_value,
                    variants.cases.clone(),
                );
                Ok(())
            }

            _ => {
                Err(format!("Expected a Variant type for the variant {}, but obtained {:?}", variant_name, analysed_type))
            }
        }
    }

    // Separate variant
    pub(crate) async fn run_call_instruction(
        parsed_function_name: ParsedFunctionName,
        argument_size: usize,
        interpreter: &mut Interpreter,
    ) -> Result<(), String> {

        let last_n_elements = interpreter
            .stack
            .pop_n(argument_size)
            .ok_or("Failed to get values from the stack".to_string())?;

        let type_anntoated_values = last_n_elements
            .iter()
            .map(|interpreter_result| {
                interpreter_result
                    .get_val()
                    .ok_or("Failed to get value from the stack".to_string())
            })
            .collect::<Result<Vec<TypeAnnotatedValue>, String>>()?;

        let result = interpreter
            .env
            .invoke_worker_function_async(parsed_function_name, type_anntoated_values)
            .await?;

        // TODO refactor
        let interpreter_result = match result {
            TypeAnnotatedValue::Tuple(TypedTuple { value, .. }) if value.is_empty() => {
                Ok(RibInterpreterResult::Unit)
            }
            TypeAnnotatedValue::Tuple(TypedTuple { value, .. }) if value.len() == 1 => {
                let inner = value[0]
                    .clone()
                    .type_annotated_value
                    .ok_or("Internal Error. Unexpected empty result")?;
                Ok(RibInterpreterResult::Val(inner))
            }
            _ => Err("Named multiple results are not supported yet".to_string())
        };

        interpreter.stack.push(interpreter_result?);

        Ok(())
    }

    pub(crate) fn run_jump_if_false_instruction(
        instruction_id: InstructionId,
        instruction_stack: &mut VecDeque<RibIR>,
        interpreter_stack: &mut InterpreterStack,
    ) -> Result<(), String> {
        let condition = interpreter_stack.pop().ok_or(
            "Failed to get a value from the stack to do the comparison operation".to_string(),
        )?;

        let predicate_bool = condition
            .get_bool()
            .ok_or("Expected a boolean value".to_string())?;

        if !predicate_bool {
            drain_instruction_stack_until_label(instruction_id, instruction_stack);
        }

        Ok(())
    }

    pub(crate) fn run_deconstruct_instruction(
        interpreter_stack: &mut InterpreterStack,
    ) -> Result<(), String> {
        let value = interpreter_stack
            .pop()
            .ok_or("Failed to get a value from the stack to unwrap".to_string())?;

        let unwrapped_value = value
            .unwrap()
            .ok_or("Failed to unwrap the value".to_string())?;

        interpreter_stack.push_val(unwrapped_value);
        Ok(())
    }

    pub(crate) fn run_get_tag_instruction(
        interpreter_stack: &mut InterpreterStack,
    ) -> Result<(), String> {
        let value = interpreter_stack
            .pop_val()
            .ok_or("Failed to get a tag value from the stack to unwrap".to_string())?;


        let tag = match value {
            TypeAnnotatedValue::Variant(variant) => {
                variant.case_name
            }
            TypeAnnotatedValue::Option(option) => {
                match option.value {
                    Some(_) => "some".to_string(),
                    None => "none".to_string()
                }
            }
            TypeAnnotatedValue::Result(result) => {
                match result.result_value {
                    Some(result_value) => match result_value {
                        ResultValue::OkValue(_) => "ok".to_string(),
                        ResultValue::ErrorValue(_) => "err".to_string()
                    }
                    None => "err".to_string()
                }
            }
            _ => "untagged".to_string()
        };

        interpreter_stack.push_val(TypeAnnotatedValue::Str(tag));
        Ok(())
    }


    pub(crate) fn run_create_some_instruction(
        interpreter_stack: &mut InterpreterStack,
        analysed_type: AnalysedType,
    ) -> Result<(), String> {
        let value = interpreter_stack
            .pop_val()
            .ok_or("Failed to get a value from the stack to wrap in Some".to_string())?;

        match analysed_type {
            AnalysedType::Option(analysed_type) => {
                interpreter_stack.push_some(value, analysed_type.inner.deref());
                Ok(())
            }
            _ => Err("Expected an Option type".to_string()),
        }
    }

    pub(crate) fn run_create_none_instruction(
        interpreter_stack: &mut InterpreterStack,
        analysed_type: Option<AnalysedType>,
    ) -> Result<(), String> {
        match analysed_type {
            Some(AnalysedType::Option(_)) | None => {
                interpreter_stack.push_none(analysed_type);
                Ok(())
            }
            _ => Err("Expected an Option type".to_string()),
        }
    }

    pub(crate) fn run_create_ok_instruction(
        interpreter_stack: &mut InterpreterStack,
        analysed_type: AnalysedType,
    ) -> Result<(), String> {
        let value = interpreter_stack
            .pop_val()
            .ok_or("Failed to get a value from the stack to wrap in Ok".to_string())?;

        match analysed_type {
            AnalysedType::Result(TypeResult { ok, err }) => {
                interpreter_stack.push_ok(value, ok.as_deref(), err.as_deref());
                Ok(())
            }
            _ => Err("Expected a Result type".to_string()),
        }
    }

    pub(crate) fn run_concat_instruction(
        interpreter_stack: &mut InterpreterStack,
        arg_size: usize
    ) -> Result<(), String> {
        let last_n_elements = interpreter_stack
            .pop_n(arg_size)
            .ok_or("Failed to get values from the stack".to_string())?;

        let type_anntoated_values = last_n_elements
            .iter()
            .map(|interpreter_result| {
                interpreter_result
                    .get_val()
                    .ok_or("Failed to get value from the stack".to_string())
            })
            .collect::<Result<Vec<TypeAnnotatedValue>, String>>()?;

        let mut str = String::new();
        for value in type_anntoated_values {
            let result = value.get_literal().ok_or("Expected a literal value".to_string())?.as_string();
            str.push_str(&result);
        }

        interpreter_stack.push_val(TypeAnnotatedValue::Str(str));

        Ok(())

    }

    pub(crate) fn run_create_err_instruction(
        interpreter_stack: &mut InterpreterStack,
        analysed_type: AnalysedType,
    ) -> Result<(), String> {
        let value = interpreter_stack
            .pop_val()
            .ok_or("Failed to get a value from the stack to wrap in Err".to_string())?;

        match analysed_type {
            AnalysedType::Result(TypeResult { ok, err }) => {
                interpreter_stack.push_err(value, ok.as_deref(), err.as_deref());
                Ok(())
            }
            _ => Err("Expected a Result type".to_string()),
        }
    }

    pub(crate) fn drain_instruction_stack_until_label(
        instruction_id: InstructionId,
        instruction_stack: &mut VecDeque<RibIR>,
    ) {
        while let Some(instruction) = instruction_stack.pop_front() {
            match instruction {
                RibIR::Label(label_instruction_id) => {
                    if label_instruction_id == instruction_id {
                        break;
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod interpreter_tests {
    use super::*;
    use crate::{InstructionId, VariableId};
    use golem_wasm_ast::analysis::{AnalysedType, NameTypePair, TypeList, TypeRecord, TypeS32};
    use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
    use golem_wasm_rpc::protobuf::{NameValuePair, TypedList, TypedRecord};

    #[tokio::test]
    async fn test_interpreter_for_literal() {
        let mut interpreter = Interpreter::default();

        let instructions = RibByteCode {
            instructions: vec![RibIR::PushLit(TypeAnnotatedValue::S32(1))],
        };

        let result = interpreter.run(instructions).await.unwrap();
        assert_eq!(result.get_val().unwrap(), TypeAnnotatedValue::S32(1));
    }

    #[tokio::test]
    async fn test_interpreter_for_equal_to() {
        let mut interpreter = Interpreter::default();

        let instructions = RibByteCode {
            instructions: vec![
                RibIR::PushLit(TypeAnnotatedValue::S32(1)),
                RibIR::PushLit(TypeAnnotatedValue::U32(1)),
                RibIR::EqualTo,
            ],
        };

        let result = interpreter.run(instructions).await.unwrap();
        assert_eq!(result.get_bool().unwrap(), true);
    }

    #[tokio::test]
    async fn test_interpreter_for_greater_than() {
        let mut interpreter = Interpreter::default();

        let instructions = RibByteCode {
            instructions: vec![
                RibIR::PushLit(TypeAnnotatedValue::S32(1)),
                RibIR::PushLit(TypeAnnotatedValue::U32(2)),
                RibIR::GreaterThan,
            ],
        };

        let result = interpreter.run(instructions).await.unwrap();
        assert_eq!(result.get_bool().unwrap(), true);
    }

    #[tokio::test]
    async fn test_interpreter_for_less_than() {
        let mut interpreter = Interpreter::default();

        let instructions = RibByteCode {
            instructions: vec![
                RibIR::PushLit(TypeAnnotatedValue::S32(2)),
                RibIR::PushLit(TypeAnnotatedValue::U32(1)),
                RibIR::LessThan,
            ],
        };

        let result = interpreter.run(instructions).await.unwrap();
        assert_eq!(result.get_bool().unwrap(), true);
    }

    #[tokio::test]
    async fn test_interpreter_for_greater_than_or_equal_to() {
        let mut interpreter = Interpreter::default();

        let instructions = RibByteCode {
            instructions: vec![
                RibIR::PushLit(TypeAnnotatedValue::S32(2)),
                RibIR::PushLit(TypeAnnotatedValue::U32(3)),
                RibIR::GreaterThanOrEqualTo,
            ],
        };

        let result = interpreter.run(instructions).await.unwrap();
        assert_eq!(result.get_bool().unwrap(), true);
    }

    #[tokio::test]
    async fn test_interpreter_for_less_than_or_equal_to() {
        let mut interpreter = Interpreter::default();

        let instructions = RibByteCode {
            instructions: vec![
                RibIR::PushLit(TypeAnnotatedValue::S32(2)), // rhs
                RibIR::PushLit(TypeAnnotatedValue::S32(1)), // lhs
                RibIR::LessThanOrEqualTo,
            ],
        };

        let result = interpreter.run(instructions).await.unwrap();
        assert_eq!(result.get_bool().unwrap(), false);
    }

    #[tokio::test]
    async fn test_interpreter_for_assign_and_load_var() {
        let mut interpreter = Interpreter::default();

        let instructions = RibByteCode {
            instructions: vec![
                RibIR::PushLit(TypeAnnotatedValue::S32(1)),
                RibIR::AssignVar(VariableId::local_with_no_id("x")),
                RibIR::LoadVar(VariableId::local_with_no_id("x")),
            ],
        };

        let result = interpreter.run(instructions).await.unwrap();
        assert_eq!(result.get_val().unwrap(), TypeAnnotatedValue::S32(1));
    }

    #[tokio::test]
    async fn test_interpreter_for_jump() {
        let mut interpreter = Interpreter::default();

        let instructions = RibByteCode {
            instructions: vec![
                RibIR::Jump(InstructionId::init()),
                RibIR::PushLit(TypeAnnotatedValue::S32(1)),
                RibIR::Label(InstructionId::init()),
            ],
        };

        let result = interpreter.run(instructions).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_interpreter_for_jump_if_false() {
        let mut interpreter = Interpreter::default();

        let id = InstructionId::init().increment_mut();

        let instructions = RibByteCode {
            instructions: vec![
                RibIR::PushLit(TypeAnnotatedValue::Bool(false)),
                RibIR::JumpIfFalse(id.clone()),
                RibIR::PushLit(TypeAnnotatedValue::S32(1)),
                RibIR::Label(id),
            ],
        };

        let result = interpreter.run(instructions).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_interpreter_for_record() {
        let mut interpreter = Interpreter::default();

        let instructions = RibByteCode {
            instructions: vec![
                RibIR::PushLit(TypeAnnotatedValue::S32(2)),
                RibIR::PushLit(TypeAnnotatedValue::S32(1)),
                RibIR::CreateAndPushRecord(AnalysedType::Record(TypeRecord {
                    fields: vec![
                        NameTypePair {
                            name: "x".to_string(),
                            typ: AnalysedType::S32(TypeS32),
                        },
                        NameTypePair {
                            name: "y".to_string(),
                            typ: AnalysedType::S32(TypeS32),
                        },
                    ],
                })),
                RibIR::UpdateRecord("x".to_string()),
                RibIR::UpdateRecord("y".to_string()),
            ],
        };

        let result = interpreter.run(instructions).await.unwrap();
        let expected = TypeAnnotatedValue::Record(TypedRecord {
            value: vec![
                NameValuePair {
                    name: "x".to_string(),
                    value: Some(golem_wasm_rpc::protobuf::TypeAnnotatedValue {
                        type_annotated_value: Some(TypeAnnotatedValue::S32(1)),
                    }),
                },
                NameValuePair {
                    name: "y".to_string(),
                    value: Some(golem_wasm_rpc::protobuf::TypeAnnotatedValue {
                        type_annotated_value: Some(TypeAnnotatedValue::S32(2)),
                    }),
                },
            ],
            typ: vec![
                golem_wasm_ast::analysis::protobuf::NameTypePair {
                    name: "x".to_string(),
                    typ: Some(golem_wasm_ast::analysis::protobuf::Type::from(
                        &AnalysedType::S32(TypeS32),
                    )),
                },
                golem_wasm_ast::analysis::protobuf::NameTypePair {
                    name: "y".to_string(),
                    typ: Some(golem_wasm_ast::analysis::protobuf::Type::from(
                        &AnalysedType::S32(TypeS32),
                    )),
                },
            ],
        });
        assert_eq!(result.get_val().unwrap(), expected);
    }

    #[tokio::test]
    async fn test_interpreter_for_sequence() {
        let mut interpreter = Interpreter::default();

        let instructions = RibByteCode {
            instructions: vec![
                RibIR::PushLit(TypeAnnotatedValue::S32(2)),
                RibIR::PushLit(TypeAnnotatedValue::S32(1)),
                RibIR::PushList(
                    AnalysedType::List(TypeList {
                        inner: Box::new(AnalysedType::S32(TypeS32)),
                    }),
                    2,
                ),
            ],
        };

        let result = interpreter.run(instructions).await.unwrap();
        let expected = TypeAnnotatedValue::List(TypedList {
            values: vec![
                golem_wasm_rpc::protobuf::TypeAnnotatedValue {
                    type_annotated_value: Some(TypeAnnotatedValue::S32(1)),
                },
                golem_wasm_rpc::protobuf::TypeAnnotatedValue {
                    type_annotated_value: Some(TypeAnnotatedValue::S32(2)),
                },
            ],
            typ: Some(golem_wasm_ast::analysis::protobuf::Type::from(
                &AnalysedType::List(TypeList {
                    inner: Box::new(AnalysedType::S32(TypeS32)),
                }),
            )),
        });
        assert_eq!(result.get_val().unwrap(), expected);
    }

    #[tokio::test]
    async fn test_interpreter_for_select_field() {
        let mut interpreter = Interpreter::default();

        let instructions = RibByteCode {
            instructions: vec![
                RibIR::PushLit(TypeAnnotatedValue::S32(1)),
                RibIR::PushLit(TypeAnnotatedValue::S32(2)),
                RibIR::CreateAndPushRecord(AnalysedType::Record(TypeRecord {
                    fields: vec![NameTypePair {
                        name: "x".to_string(),
                        typ: AnalysedType::S32(TypeS32),
                    }],
                })),
                RibIR::UpdateRecord("x".to_string()),
                RibIR::SelectField("x".to_string()),
            ],
        };

        let result = interpreter.run(instructions).await.unwrap();
        assert_eq!(result.get_val().unwrap(), TypeAnnotatedValue::S32(2));
    }

    #[tokio::test]
    async fn test_interpreter_for_select_index() {
        let mut interpreter = Interpreter::default();

        let instructions = RibByteCode {
            instructions: vec![
                RibIR::PushLit(TypeAnnotatedValue::S32(1)),
                RibIR::PushLit(TypeAnnotatedValue::S32(2)),
                RibIR::PushList(
                    AnalysedType::List(TypeList {
                        inner: Box::new(AnalysedType::S32(TypeS32)),
                    }),
                    2,
                ),
                RibIR::SelectIndex(0),
            ],
        };

        let result = interpreter.run(instructions).await.unwrap();
        assert_eq!(result.get_val().unwrap(), TypeAnnotatedValue::S32(2));
    }
}
