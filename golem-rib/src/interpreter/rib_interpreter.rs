// Copyright 2024 Golem Cloud
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

use crate::interpreter::env::{InterpreterEnv, RibFunctionInvoke};
use crate::interpreter::instruction_cursor::RibByteCodeCursor;
use crate::interpreter::stack::InterpreterStack;
use crate::{RibByteCode, RibIR, RibInput, RibResult};

pub struct Interpreter {
    pub input: RibInput,
    pub invoke: RibFunctionInvoke,
}

impl Default for Interpreter {
    fn default() -> Self {
        Interpreter {
            input: RibInput::default(),
            invoke: internal::default_worker_invoke_async(),
        }
    }
}

impl Interpreter {
    pub fn new(input: &RibInput, invoke: RibFunctionInvoke) -> Self {
        Interpreter {
            input: input.clone(),
            invoke,
        }
    }

    // Interpreter that's not expected to call a side-effecting function call.
    // All it needs is environment with the required variables to evaluate the Rib script
    pub fn pure(input: &RibInput) -> Self {
        Interpreter {
            input: input.clone(),
            invoke: internal::default_worker_invoke_async(),
        }
    }

    pub async fn run(&mut self, instructions0: RibByteCode) -> Result<RibResult, String> {
        let mut byte_code_cursor = RibByteCodeCursor::from_rib_byte_code(instructions0);
        let mut stack = InterpreterStack::new();
        let mut interpreter_env = InterpreterEnv::from(&self.input, &self.invoke);

        while let Some(instruction) = byte_code_cursor.get_instruction() {
            match instruction {
                RibIR::PushLit(val) => {
                    stack.push_val(val);
                }

                RibIR::PushFlag(val) => {
                    stack.push_val(val);
                }

                RibIR::CreateAndPushRecord(analysed_type) => {
                    internal::run_create_record_instruction(analysed_type, &mut stack)?;
                }

                RibIR::UpdateRecord(field_name) => {
                    internal::run_update_record_instruction(field_name, &mut stack)?;
                }

                RibIR::PushList(analysed_type, arg_size) => {
                    internal::run_push_list_instruction(arg_size, analysed_type, &mut stack)?;
                }

                RibIR::EqualTo => {
                    internal::run_compare_instruction(&mut stack, |left, right| left == right)?;
                }

                RibIR::GreaterThan => {
                    internal::run_compare_instruction(&mut stack, |left, right| left > right)?;
                }

                RibIR::LessThan => {
                    internal::run_compare_instruction(&mut stack, |left, right| left < right)?;
                }

                RibIR::GreaterThanOrEqualTo => {
                    internal::run_compare_instruction(&mut stack, |left, right| left >= right)?;
                }

                RibIR::LessThanOrEqualTo => {
                    internal::run_compare_instruction(&mut stack, |left, right| left <= right)?;
                }
                RibIR::Add(analysed_type) => {
                    internal::run_math_instruction(
                        &mut stack,
                        |left, right| left + right,
                        &analysed_type,
                    )?;
                }
                RibIR::Subtract(analysed_type) => {
                    internal::run_math_instruction(
                        &mut stack,
                        |left, right| left - right,
                        &analysed_type,
                    )?;
                }
                RibIR::Divide(analysed_type) => {
                    internal::run_math_instruction(
                        &mut stack,
                        |left, right| left - right,
                        &analysed_type,
                    )?;
                }
                RibIR::Multiply(analysed_type) => {
                    internal::run_math_instruction(
                        &mut stack,
                        |left, right| left * right,
                        &analysed_type,
                    )?;
                }

                RibIR::AssignVar(variable_id) => {
                    internal::run_assign_var_instruction(
                        variable_id,
                        &mut stack,
                        &mut interpreter_env,
                    )?;
                }

                RibIR::LoadVar(variable_id) => {
                    internal::run_load_var_instruction(
                        variable_id,
                        &mut stack,
                        &mut interpreter_env,
                    )?;
                }

                RibIR::IsEmpty => {
                    internal::run_is_empty_instruction(&mut stack)?;
                }

                RibIR::JumpIfFalse(instruction_id) => {
                    internal::run_jump_if_false_instruction(
                        instruction_id,
                        &mut byte_code_cursor,
                        &mut stack,
                    )?;
                }

                RibIR::SelectField(field_name) => {
                    internal::run_select_field_instruction(field_name, &mut stack)?;
                }

                RibIR::SelectIndex(index) => {
                    internal::run_select_index_instruction(&mut stack, index)?;
                }

                RibIR::CreateFunctionName(site, function_type) => {
                    internal::run_create_function_name_instruction(
                        site,
                        function_type,
                        &mut stack,
                    )?;
                }

                RibIR::InvokeFunction(arg_size, _) => {
                    internal::run_call_instruction(arg_size, &mut stack, &mut interpreter_env)
                        .await?;
                }

                RibIR::PushVariant(variant_name, analysed_type) => {
                    internal::run_variant_construction_instruction(
                        variant_name,
                        analysed_type,
                        &mut stack,
                    )
                    .await?;
                }

                RibIR::PushEnum(enum_name, analysed_type) => {
                    internal::run_push_enum_instruction(&mut stack, enum_name, analysed_type)?;
                }

                RibIR::Throw(message) => {
                    return Err(message);
                }

                RibIR::GetTag => {
                    internal::run_get_tag_instruction(&mut stack)?;
                }

                RibIR::Deconstruct => {
                    internal::run_deconstruct_instruction(&mut stack)?;
                }

                RibIR::Jump(instruction_id) => {
                    byte_code_cursor.move_to(&instruction_id).ok_or(format!(
                        "Internal error. Failed to move to label {}",
                        instruction_id.index
                    ))?;
                }

                RibIR::PushSome(analysed_type) => {
                    internal::run_create_some_instruction(&mut stack, analysed_type)?;
                }
                RibIR::PushNone(analysed_type) => {
                    internal::run_create_none_instruction(&mut stack, analysed_type)?;
                }
                RibIR::PushOkResult(analysed_type) => {
                    internal::run_create_ok_instruction(&mut stack, analysed_type)?;
                }
                RibIR::PushErrResult(analysed_type) => {
                    internal::run_create_err_instruction(&mut stack, analysed_type)?;
                }
                RibIR::Concat(arg_size) => {
                    internal::run_concat_instruction(&mut stack, arg_size)?;
                }
                RibIR::PushTuple(analysed_type, arg_size) => {
                    internal::run_push_tuple_instruction(arg_size, analysed_type, &mut stack)?;
                }
                RibIR::Negate => {
                    internal::run_negate_instruction(&mut stack)?;
                }

                RibIR::Label(_) => {}

                RibIR::And => {
                    internal::run_and_instruction(&mut stack)?;
                }

                RibIR::Or => {
                    internal::run_or_instruction(&mut stack)?;
                }
                RibIR::ListToIterator => {
                    internal::run_list_to_iterator_instruction(&mut stack)?;
                }
                RibIR::CreateSink(analysed_type) => {
                    internal::run_create_sink_instruction(&mut stack, &analysed_type)?
                }
                RibIR::AdvanceIterator => {
                    internal::run_advance_iterator_instruction(&mut stack)?;
                }
                RibIR::PushToSink => {
                    internal::run_push_to_sink_instruction(&mut stack)?;
                }
                RibIR::SinkToList => {
                    internal::run_sink_to_list_instruction(&mut stack)?;
                }
            }
        }

        let stack_value = stack
            .pop()
            .ok_or("Empty stack after running the instructions".to_string())?;

        let rib_result = RibResult::from_rib_interpreter_stack_value(&stack_value)
            .ok_or("Failed to obtain a valid result from rib execution".to_string())?;

        Ok(rib_result)
    }
}

mod internal {
    use crate::interpreter::env::{EnvironmentKey, InterpreterEnv};
    use crate::interpreter::interpreter_stack_value::RibInterpreterStackValue;
    use crate::interpreter::literal::LiteralValue;
    use crate::interpreter::stack::InterpreterStack;
    use crate::{
        CoercedNumericValue, FunctionReferenceType, GetLiteralValue, InstructionId,
        ParsedFunctionName, ParsedFunctionReference, ParsedFunctionSite, RibFunctionInvoke,
        VariableId,
    };
    use golem_wasm_ast::analysis::AnalysedType;
    use golem_wasm_ast::analysis::TypeResult;
    use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
    use golem_wasm_rpc::protobuf::typed_result::ResultValue;
    use golem_wasm_rpc::protobuf::{NameValuePair, TypedRecord, TypedTuple};
    use golem_wasm_rpc::type_annotated_value_to_string;

    use crate::interpreter::instruction_cursor::RibByteCodeCursor;
    use golem_wasm_ast::analysis::analysed_type::str;
    use std::ops::Deref;
    use std::sync::Arc;

    pub(crate) fn default_worker_invoke_async() -> RibFunctionInvoke {
        Arc::new(|_, _| {
            Box::pin(async {
                Ok(TypeAnnotatedValue::Tuple(TypedTuple {
                    typ: vec![],
                    value: vec![],
                }))
            })
        })
    }

    pub(crate) fn run_is_empty_instruction(
        interpreter_stack: &mut InterpreterStack,
    ) -> Result<(), String> {
        let rib_result = interpreter_stack.pop().ok_or(
            "Internal Error: Failed to get a value from the stack to do check is_empty".to_string(),
        )?;

        let bool_opt = match rib_result {
            RibInterpreterStackValue::Val(TypeAnnotatedValue::List(typed_list)) => {
                Some(typed_list.values.is_empty())
            }
            RibInterpreterStackValue::Iterator(iter) => {
                let mut peekable_iter = iter.peekable();
                let result = peekable_iter.peek().is_some();
                interpreter_stack.push(RibInterpreterStackValue::Iterator(Box::new(peekable_iter)));
                Some(result)
            }
            RibInterpreterStackValue::Sink(values, analysed_type) => {
                let possible_iterator = interpreter_stack
                    .pop()
                    .ok_or("Internal Error: Expecting an iterator to check is empty".to_string())?;

                match possible_iterator {
                    RibInterpreterStackValue::Iterator(iter) => {
                        let mut peekable_iter = iter.peekable();
                        let result = peekable_iter.peek().is_some();
                        interpreter_stack
                            .push(RibInterpreterStackValue::Iterator(Box::new(peekable_iter)));
                        interpreter_stack
                            .push(RibInterpreterStackValue::Sink(values, analysed_type));
                        Some(result)
                    }

                    _ => None,
                }
            }
            RibInterpreterStackValue::Val(_) => None,
            RibInterpreterStackValue::Unit => None,
        };

        let bool = bool_opt.ok_or("Internal Error: Failed to run instruction is_empty")?;
        interpreter_stack.push_val(TypeAnnotatedValue::Bool(bool));
        Ok(())
    }

    pub(crate) fn run_jump_if_false_instruction(
        instruction_id: InstructionId,
        instruction_stack: &mut RibByteCodeCursor,
        interpreter_stack: &mut InterpreterStack,
    ) -> Result<(), String> {
        let rib_result = interpreter_stack.pop().ok_or(
            "Internal Error: Failed to get a value from the stack to do the comparison operation"
                .to_string(),
        )?;

        let predicate_bool = rib_result
            .get_bool()
            .ok_or("Internal Error: Expecting a value that can be converted to bool".to_string())?;

        if !predicate_bool {
            instruction_stack.move_to(&instruction_id).ok_or(format!(
                "Internal Error: Failed to move to the instruction at {}",
                instruction_id.index
            ))?;
        }

        Ok(())
    }

    pub(crate) fn run_list_to_iterator_instruction(
        interpreter_stack: &mut InterpreterStack,
    ) -> Result<(), String> {
        if let Some(RibInterpreterStackValue::Val(TypeAnnotatedValue::List(items))) =
            interpreter_stack.pop()
        {
            let iter = items
                .values
                .into_iter()
                .map(|x| x.clone().type_annotated_value.unwrap());

            interpreter_stack.push(RibInterpreterStackValue::Iterator(Box::new(iter)));

            Ok(())
        } else {
            Err("Internal Error: Expected a List on the stack for ListToIterator".to_string())
        }
    }

    pub(crate) fn run_create_sink_instruction(
        interpreter_stack: &mut InterpreterStack,
        analysed_type: &AnalysedType,
    ) -> Result<(), String> {
        let analysed_type = match analysed_type {
            AnalysedType::List(type_list) => type_list.clone().inner,
            _ => return Err("Expecting a list type to create sink".to_string()),
        };
        interpreter_stack.create_sink(analysed_type.deref());
        Ok(())
    }

    pub(crate) fn run_advance_iterator_instruction(
        interpreter_stack: &mut InterpreterStack,
    ) -> Result<(), String> {
        let mut rib_result = interpreter_stack
            .pop()
            .ok_or("Internal Error: Failed to advance the iterator".to_string())?;

        match &mut rib_result {
            RibInterpreterStackValue::Sink(_, _) => {
                let mut existing_iterator = interpreter_stack
                    .pop()
                    .ok_or("Internal Error: an iterator")?;

                match &mut existing_iterator {
                    RibInterpreterStackValue::Iterator(iter) => {
                        if let Some(type_annotated_value) = iter.next() {
                            interpreter_stack.push(existing_iterator);
                            interpreter_stack.push(rib_result);
                            interpreter_stack
                                .push(RibInterpreterStackValue::Val(type_annotated_value));
                            Ok(())
                        } else {
                            Err("Internal Error: Iterator has no more items".to_string())
                        }
                    }

                    _ => Err(
                        "Internal Error: A sink cannot exist without a corresponding iterator"
                            .to_string(),
                    ),
                }
            }

            RibInterpreterStackValue::Iterator(iter) => {
                if let Some(type_annotated_value) = iter.next() {
                    interpreter_stack.push(rib_result);
                    interpreter_stack.push(RibInterpreterStackValue::Val(type_annotated_value));
                    Ok(())
                } else {
                    Err("Internal Error: Iterator has no more items".to_string())
                }
            }
            _ => Err("Internal Error: Expected an Iterator on the stack".to_string()),
        }
    }

    pub(crate) fn run_push_to_sink_instruction(
        interpreter_stack: &mut InterpreterStack,
    ) -> Result<(), String> {
        let last_value = interpreter_stack.pop_val();
        match last_value {
            Some(val) => {
                interpreter_stack.push_to_sink(val)?;

                Ok(())
            }
            _ => Err("Failed to push values to sink".to_string()),
        }
    }

    pub(crate) fn run_sink_to_list_instruction(
        interpreter_stack: &mut InterpreterStack,
    ) -> Result<(), String> {
        let result = interpreter_stack
            .pop_sink()
            .ok_or("Failed to retrieve items from sink")?;
        interpreter_stack.push_list(result, &str());

        Ok(())
    }

    pub(crate) fn run_assign_var_instruction(
        variable_id: VariableId,
        interpreter_stack: &mut InterpreterStack,
        interpreter_env: &mut InterpreterEnv,
    ) -> Result<(), String> {
        let value = interpreter_stack
            .pop()
            .ok_or("Expected a value on the stack before assigning a variable".to_string())?;
        let env_key = EnvironmentKey::from(variable_id);

        interpreter_env.insert(env_key, value);
        Ok(())
    }

    pub(crate) fn run_load_var_instruction(
        variable_id: VariableId,
        interpreter_stack: &mut InterpreterStack,
        interpreter_env: &mut InterpreterEnv,
    ) -> Result<(), String> {
        let env_key = EnvironmentKey::from(variable_id.clone());
        let value = interpreter_env.lookup(&env_key).ok_or(format!(
            "Variable `{}` not found during evaluation of expression",
            variable_id
        ))?;

        match value {
            RibInterpreterStackValue::Unit => {
                interpreter_stack.push(RibInterpreterStackValue::Unit);
            }
            RibInterpreterStackValue::Val(val) => interpreter_stack.push_val(val.clone()),
            RibInterpreterStackValue::Iterator(_) => {
                return Err("Unable to assign an iterator to a variable".to_string())
            }
            RibInterpreterStackValue::Sink(_, _) => {
                return Err("Unable to assign a sink to a variable".to_string())
            }
        }

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
                            .ok_or("Internal Error: Failed to construct list".to_string())
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
                            .ok_or("Internal Error: Failed to construct tuple".to_string())
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

        let result = value
            .get_literal()
            .and_then(|literal| literal.get_bool())
            .ok_or("Failed to get a boolean value from the stack to negate".to_string())?;

        interpreter_stack.push_val(TypeAnnotatedValue::Bool(!result));
        Ok(())
    }

    pub(crate) fn run_and_instruction(
        interpreter_stack: &mut InterpreterStack,
    ) -> Result<(), String> {
        let left = interpreter_stack
            .pop()
            .ok_or("Internal Error: Failed to get LHS &&".to_string())?;
        let right = interpreter_stack
            .pop()
            .ok_or("Internal Error: Failed to get RHS of &&".to_string())?;

        let result = left.compare(&right, |a, b| match (a.get_bool(), b.get_bool()) {
            (Some(a), Some(b)) => a && b,
            _ => false,
        })?;

        interpreter_stack.push(result);

        Ok(())
    }

    pub(crate) fn run_or_instruction(
        interpreter_stack: &mut InterpreterStack,
    ) -> Result<(), String> {
        let left = interpreter_stack
            .pop()
            .ok_or("Internal Error: Failed to get LHS &&".to_string())?;
        let right = interpreter_stack
            .pop()
            .ok_or("Internal Error: Failed to get RHS of &&".to_string())?;

        let result = left.compare(&right, |a, b| match (a.get_bool(), b.get_bool()) {
            (Some(a), Some(b)) => a || b,
            _ => false,
        })?;

        interpreter_stack.push(result);

        Ok(())
    }

    pub(crate) fn run_math_instruction(
        interpreter_stack: &mut InterpreterStack,
        compare_fn: fn(CoercedNumericValue, CoercedNumericValue) -> CoercedNumericValue,
        target_numerical_type: &AnalysedType,
    ) -> Result<(), String> {
        let left = interpreter_stack.pop().ok_or(
            "Empty stack and failed to get a value to do the comparison operation".to_string(),
        )?;
        let right = interpreter_stack.pop().ok_or(
            "Failed to get a value from the stack to do the comparison operation".to_string(),
        )?;

        let result = left.evaluate_math_op(&right, compare_fn)?;
        let numerical_type = result.cast_to(target_numerical_type).ok_or(format!(
            "Failed to cast number {} to {:?}",
            result, target_numerical_type
        ))?;

        interpreter_stack.push_val(numerical_type);

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
            RibInterpreterStackValue::Val(TypeAnnotatedValue::Record(record)) => {
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
            RibInterpreterStackValue::Val(TypeAnnotatedValue::List(typed_list)) => {
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
            RibInterpreterStackValue::Val(TypeAnnotatedValue::Tuple(typed_tuple)) => {
                let value = typed_tuple
                    .value
                    .get(index)
                    .ok_or(format!("Index {} not found in the tuple", index))?
                    .clone();

                let inner_type_annotated_value = value
                    .type_annotated_value
                    .ok_or("Field value not found".to_string())?;

                interpreter_stack.push_val(inner_type_annotated_value);
                Ok(())
            }
            result => Err(format!(
                "Expected a sequence value or tuple to select an index. But obtained {:?}",
                result
            )),
        }
    }

    pub(crate) fn run_push_enum_instruction(
        interpreter_stack: &mut InterpreterStack,
        enum_name: String,
        analysed_type: AnalysedType,
    ) -> Result<(), String> {
        match analysed_type {
            AnalysedType::Enum(typed_enum) => {
                interpreter_stack.push_enum(enum_name, typed_enum.cases);
                Ok(())
            }
            _ => Err(format!(
                "Expected a enum type for {}, but obtained {:?}",
                enum_name, analysed_type
            )),
        }
    }

    pub(crate) async fn run_variant_construction_instruction(
        variant_name: String,
        analysed_type: AnalysedType,
        interpreter_stack: &mut InterpreterStack,
    ) -> Result<(), String> {
        match analysed_type {
            AnalysedType::Variant(variants) => {
                let variant = variants
                    .cases
                    .iter()
                    .find(|name| name.name == variant_name)
                    .ok_or(format!("Unknown variant {} not found", variant_name))?;

                let variant_arg_typ = variant.typ.clone();

                let arg_value =
                    match variant_arg_typ {
                        Some(_) => Some(interpreter_stack.pop_val().ok_or(
                            "Failed to get the variant argument from the stack".to_string(),
                        )?),
                        None => None,
                    };

                interpreter_stack.push_variant(
                    variant_name.clone(),
                    arg_value,
                    variants.cases.clone(),
                );
                Ok(())
            }

            _ => Err(format!(
                "Expected a Variant type for the variant {}, but obtained {:?}",
                variant_name, analysed_type
            )),
        }
    }

    pub(crate) fn run_create_function_name_instruction(
        site: ParsedFunctionSite,
        function_type: FunctionReferenceType,
        interpreter_stack: &mut InterpreterStack,
    ) -> Result<(), String> {
        match function_type {
            FunctionReferenceType::Function { function } => {
                let parsed_function_name = ParsedFunctionName {
                    site,
                    function: ParsedFunctionReference::Function { function },
                };

                interpreter_stack
                    .push_val(TypeAnnotatedValue::Str(parsed_function_name.to_string()));
            }

            FunctionReferenceType::RawResourceConstructor { resource } => {
                let parsed_function_name = ParsedFunctionName {
                    site,
                    function: ParsedFunctionReference::RawResourceConstructor { resource },
                };

                interpreter_stack
                    .push_val(TypeAnnotatedValue::Str(parsed_function_name.to_string()));
            }
            FunctionReferenceType::RawResourceDrop { resource } => {
                let parsed_function_name = ParsedFunctionName {
                    site,
                    function: ParsedFunctionReference::RawResourceDrop { resource },
                };

                interpreter_stack
                    .push_val(TypeAnnotatedValue::Str(parsed_function_name.to_string()));
            }
            FunctionReferenceType::RawResourceMethod { resource, method } => {
                let parsed_function_name = ParsedFunctionName {
                    site,
                    function: ParsedFunctionReference::RawResourceMethod { resource, method },
                };

                interpreter_stack
                    .push_val(TypeAnnotatedValue::Str(parsed_function_name.to_string()));
            }
            FunctionReferenceType::RawResourceStaticMethod { resource, method } => {
                let parsed_function_name = ParsedFunctionName {
                    site,
                    function: ParsedFunctionReference::RawResourceStaticMethod { resource, method },
                };

                interpreter_stack
                    .push_val(TypeAnnotatedValue::Str(parsed_function_name.to_string()));
            }
            FunctionReferenceType::IndexedResourceConstructor { resource, arg_size } => {
                let last_n_elements = interpreter_stack
                    .pop_n(arg_size)
                    .ok_or("Failed to get values from the stack".to_string())?;

                let type_annotated_value = last_n_elements
                    .iter()
                    .map(|interpreter_result| {
                        interpreter_result
                            .get_val()
                            .ok_or("Internal Error: Failed to construct resource".to_string())
                    })
                    .collect::<Result<Vec<TypeAnnotatedValue>, String>>()?;

                let parsed_function_name = ParsedFunctionName {
                    site,
                    function: ParsedFunctionReference::IndexedResourceConstructor {
                        resource,
                        resource_params: type_annotated_value
                            .iter()
                            .map(type_annotated_value_to_string)
                            .collect::<Result<Vec<String>, String>>()?,
                    },
                };

                interpreter_stack
                    .push_val(TypeAnnotatedValue::Str(parsed_function_name.to_string()));
            }
            FunctionReferenceType::IndexedResourceMethod {
                resource,
                arg_size,
                method,
            } => {
                let last_n_elements = interpreter_stack
                    .pop_n(arg_size)
                    .ok_or("Failed to get values from the stack".to_string())?;

                let type_anntoated_values = last_n_elements
                    .iter()
                    .map(|interpreter_result| {
                        interpreter_result.get_val().ok_or(
                            "Internal Error: Failed to call indexed resource method".to_string(),
                        )
                    })
                    .collect::<Result<Vec<TypeAnnotatedValue>, String>>()?;

                let parsed_function_name = ParsedFunctionName {
                    site,
                    function: ParsedFunctionReference::IndexedResourceMethod {
                        resource,
                        resource_params: type_anntoated_values
                            .iter()
                            .map(type_annotated_value_to_string)
                            .collect::<Result<Vec<String>, String>>()?,
                        method,
                    },
                };

                interpreter_stack
                    .push_val(TypeAnnotatedValue::Str(parsed_function_name.to_string()));
            }
            FunctionReferenceType::IndexedResourceStaticMethod {
                resource,
                arg_size,
                method,
            } => {
                let last_n_elements = interpreter_stack.pop_n(arg_size).ok_or(
                    "Internal error: Failed to get arguments for static resource method"
                        .to_string(),
                )?;

                let type_anntoated_values = last_n_elements
                    .iter()
                    .map(|interpreter_result| {
                        interpreter_result.get_val().ok_or(
                            "Internal error: Failed to call static resource method".to_string(),
                        )
                    })
                    .collect::<Result<Vec<TypeAnnotatedValue>, String>>()?;

                let parsed_function_name = ParsedFunctionName {
                    site,
                    function: ParsedFunctionReference::IndexedResourceStaticMethod {
                        resource,
                        resource_params: type_anntoated_values
                            .iter()
                            .map(type_annotated_value_to_string)
                            .collect::<Result<Vec<String>, String>>()?,
                        method,
                    },
                };

                interpreter_stack
                    .push_val(TypeAnnotatedValue::Str(parsed_function_name.to_string()));
            }
            FunctionReferenceType::IndexedResourceDrop { resource, arg_size } => {
                let last_n_elements = interpreter_stack.pop_n(arg_size).ok_or(
                    "Internal Error: Failed to get resource parameters for indexed resource drop"
                        .to_string(),
                )?;

                let type_annotated_values = last_n_elements
                    .iter()
                    .map(|interpreter_result| {
                        interpreter_result.get_val().ok_or(
                            "Internal Error: Failed to call indexed resource drop".to_string(),
                        )
                    })
                    .collect::<Result<Vec<TypeAnnotatedValue>, String>>()?;

                let parsed_function_name = ParsedFunctionName {
                    site,
                    function: ParsedFunctionReference::IndexedResourceDrop {
                        resource,
                        resource_params: type_annotated_values
                            .iter()
                            .map(type_annotated_value_to_string)
                            .collect::<Result<Vec<String>, String>>()?,
                    },
                };

                interpreter_stack
                    .push_val(TypeAnnotatedValue::Str(parsed_function_name.to_string()));
            }
        }

        Ok(())
    }

    pub(crate) async fn run_call_instruction(
        arg_size: usize,
        interpreter_stack: &mut InterpreterStack,
        interpreter_env: &mut InterpreterEnv,
    ) -> Result<(), String> {
        let function_name = interpreter_stack
            .pop_str()
            .ok_or("Internal Error: Failed to get a function name".to_string())?;

        let last_n_elements = interpreter_stack
            .pop_n(arg_size)
            .ok_or("Internal Error: Failed to get arguments for the function call".to_string())?;

        let type_anntoated_values = last_n_elements
            .iter()
            .map(|interpreter_result| {
                interpreter_result.get_val().ok_or(format!(
                    "Internal Error: Failed to call function {}",
                    function_name
                ))
            })
            .collect::<Result<Vec<TypeAnnotatedValue>, String>>()?;

        let result = interpreter_env
            .invoke_worker_function_async(function_name, type_anntoated_values)
            .await?;

        let interpreter_result = match result {
            TypeAnnotatedValue::Tuple(TypedTuple { value, .. }) if value.is_empty() => {
                Ok(RibInterpreterStackValue::Unit)
            }
            TypeAnnotatedValue::Tuple(TypedTuple { value, .. }) if value.len() == 1 => {
                let inner = value[0]
                    .clone()
                    .type_annotated_value
                    .ok_or("Internal Error. Unexpected empty result")?;
                Ok(RibInterpreterStackValue::Val(inner))
            }
            _ => Err("Named multiple results are not supported yet".to_string()),
        };

        interpreter_stack.push(interpreter_result?);

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
            .ok_or(format!("Failed to unwrap the value {:?}", value))?;

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
            TypeAnnotatedValue::Variant(variant) => variant.case_name,
            TypeAnnotatedValue::Option(option) => match option.value {
                Some(_) => "some".to_string(),
                None => "none".to_string(),
            },
            TypeAnnotatedValue::Result(result) => match result.result_value {
                Some(result_value) => match result_value {
                    ResultValue::OkValue(_) => "ok".to_string(),
                    ResultValue::ErrorValue(_) => "err".to_string(),
                },
                None => "err".to_string(),
            },
            TypeAnnotatedValue::Enum(enum_) => enum_.value,
            _ => "untagged".to_string(),
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
        arg_size: usize,
    ) -> Result<(), String> {
        let last_n_elements = interpreter_stack
            .pop_n(arg_size)
            .ok_or("Internal Error: Failed to get arguments for concatenation".to_string())?;

        let type_annotated_values = last_n_elements
            .iter()
            .map(|interpreter_result| {
                interpreter_result
                    .get_val()
                    .ok_or("Internal Error: Failed to execute concatenation".to_string())
            })
            .collect::<Result<Vec<TypeAnnotatedValue>, String>>()?;

        let mut str = String::new();
        for value in type_annotated_values {
            let result = value
                .get_literal()
                .ok_or("Expected a literal value".to_string())?
                .as_string();
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
}

#[cfg(test)]
mod interpreter_tests {
    use test_r::test;

    use super::*;
    use crate::{InstructionId, VariableId};
    use golem_wasm_ast::analysis::analysed_type::{field, list, record, s32};
    use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
    use golem_wasm_rpc::protobuf::{NameValuePair, TypedList, TypedRecord};

    #[test]
    async fn test_interpreter_for_literal() {
        let mut interpreter = Interpreter::default();

        let instructions = RibByteCode {
            instructions: vec![RibIR::PushLit(TypeAnnotatedValue::S32(1))],
        };

        let result = interpreter.run(instructions).await.unwrap();
        assert_eq!(result.get_val().unwrap(), TypeAnnotatedValue::S32(1));
    }

    #[test]
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
        assert!(result.get_bool().unwrap());
    }

    #[test]
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
        assert!(result.get_bool().unwrap());
    }

    #[test]
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
        assert!(result.get_bool().unwrap());
    }

    #[test]
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
        assert!(result.get_bool().unwrap());
    }

    #[test]
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
        assert!(result.get_bool().unwrap());
    }

    #[test]
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

    #[test]
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

    #[test]
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

    #[test]
    async fn test_interpreter_for_record() {
        let mut interpreter = Interpreter::default();

        let instructions = RibByteCode {
            instructions: vec![
                RibIR::PushLit(TypeAnnotatedValue::S32(2)),
                RibIR::PushLit(TypeAnnotatedValue::S32(1)),
                RibIR::CreateAndPushRecord(record(vec![field("x", s32()), field("y", s32())])),
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
                    typ: Some(golem_wasm_ast::analysis::protobuf::Type::from(&s32())),
                },
                golem_wasm_ast::analysis::protobuf::NameTypePair {
                    name: "y".to_string(),
                    typ: Some(golem_wasm_ast::analysis::protobuf::Type::from(&s32())),
                },
            ],
        });
        assert_eq!(result.get_val().unwrap(), expected);
    }

    #[test]
    async fn test_interpreter_for_sequence() {
        let mut interpreter = Interpreter::default();

        let instructions = RibByteCode {
            instructions: vec![
                RibIR::PushLit(TypeAnnotatedValue::S32(2)),
                RibIR::PushLit(TypeAnnotatedValue::S32(1)),
                RibIR::PushList(list(s32()), 2),
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
            typ: Some(golem_wasm_ast::analysis::protobuf::Type::from(&s32())),
        });
        assert_eq!(result.get_val().unwrap(), expected);
    }

    #[test]
    async fn test_interpreter_for_select_field() {
        let mut interpreter = Interpreter::default();

        let instructions = RibByteCode {
            instructions: vec![
                RibIR::PushLit(TypeAnnotatedValue::S32(1)),
                RibIR::PushLit(TypeAnnotatedValue::S32(2)),
                RibIR::CreateAndPushRecord(record(vec![field("x", s32())])),
                RibIR::UpdateRecord("x".to_string()),
                RibIR::SelectField("x".to_string()),
            ],
        };

        let result = interpreter.run(instructions).await.unwrap();
        assert_eq!(result.get_val().unwrap(), TypeAnnotatedValue::S32(2));
    }

    #[test]
    async fn test_interpreter_for_select_index() {
        let mut interpreter = Interpreter::default();

        let instructions = RibByteCode {
            instructions: vec![
                RibIR::PushLit(TypeAnnotatedValue::S32(1)),
                RibIR::PushLit(TypeAnnotatedValue::S32(2)),
                RibIR::PushList(list(s32()), 2),
                RibIR::SelectIndex(0),
            ],
        };

        let result = interpreter.run(instructions).await.unwrap();
        assert_eq!(result.get_val().unwrap(), TypeAnnotatedValue::S32(2));
    }

    mod list_reduce_interpreter_tests {
        use test_r::test;

        use crate::interpreter::rib_interpreter::Interpreter;
        use crate::{compiler, Expr};
        use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;

        #[test]
        async fn test_list_reduce() {
            let mut interpreter = Interpreter::default();

            let rib_expr = r#"
          let x: list<u8> = [1, 2];

          reduce z, a in x from 0u8 {
            yield z + a;
          }

          "#;

            let expr = Expr::from_text(rib_expr).unwrap();

            let compiled = compiler::compile(&expr, &vec![]).unwrap();

            let result = interpreter
                .run(compiled.byte_code)
                .await
                .unwrap()
                .get_val()
                .unwrap();

            assert_eq!(result, TypeAnnotatedValue::U8(3));
        }

        #[test]
        async fn test_list_reduce_empty() {
            let mut interpreter = Interpreter::default();

            let rib_expr = r#"
          let x: list<u8> = [];

          reduce z, a in x from 0u8 {
            yield z + a;
          }

          "#;

            let expr = Expr::from_text(rib_expr).unwrap();

            let compiled = compiler::compile(&expr, &vec![]).unwrap();

            let result = interpreter
                .run(compiled.byte_code)
                .await
                .unwrap()
                .get_val()
                .unwrap();

            assert_eq!(result, TypeAnnotatedValue::U8(0));
        }
    }

    mod list_comprehension_interpreter_tests {
        use crate::interpreter::rib_interpreter::Interpreter;
        use crate::{compiler, Expr};
        use golem_wasm_ast::analysis::analysed_type::{list, str};
        use test_r::test;

        #[test]
        async fn test_list_comprehension() {
            let mut interpreter = Interpreter::default();

            let rib_expr = r#"
          let x = ["foo", "bar"];

          for i in x {
            yield i;
          }

          "#;

            let expr = Expr::from_text(rib_expr).unwrap();

            let compiled = compiler::compile(&expr, &vec![]).unwrap();

            let result = interpreter
                .run(compiled.byte_code)
                .await
                .unwrap()
                .get_val()
                .unwrap();

            let expected = r#"["foo", "bar"]"#;
            let expected_type_annotated_value =
                golem_wasm_rpc::type_annotated_value_from_str(&list(str()), expected).unwrap();

            assert_eq!(result, expected_type_annotated_value);
        }

        #[test]
        async fn test_list_comprehension_empty() {
            let mut interpreter = Interpreter::default();

            let rib_expr = r#"
          let x: list<str> = [];

          for i in x {
            yield i;
          }

          "#;

            let expr = Expr::from_text(rib_expr).unwrap();

            let compiled = compiler::compile(&expr, &vec![]).unwrap();

            let result = interpreter
                .run(compiled.byte_code)
                .await
                .unwrap()
                .get_val()
                .unwrap();

            let expected = r#"[]"#;
            let expected_type_annotated_value =
                golem_wasm_rpc::type_annotated_value_from_str(&list(str()), expected).unwrap();

            assert_eq!(result, expected_type_annotated_value);
        }
    }

    mod pattern_match_interpreter_tests {
        use test_r::test;

        use crate::interpreter::rib_interpreter::interpreter_tests::internal;
        use crate::interpreter::rib_interpreter::Interpreter;
        use crate::{compiler, Expr, FunctionTypeRegistry};
        use golem_wasm_ast::analysis::analysed_type::{field, record, str, tuple, u16, u64};
        use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;

        #[test]
        async fn test_pattern_match_on_option_nested() {
            let mut interpreter = Interpreter::default();

            let expr = r#"
           let x: option<option<u64>> = none;

           match x {
              some(some(t)) => t,
              some(none) => 0u64,
              none => 0u64

           }
        "#;

            let mut expr = Expr::from_text(expr).unwrap();
            expr.infer_types(&FunctionTypeRegistry::empty()).unwrap();
            let compiled = compiler::compile(&expr, &vec![]).unwrap();
            let result = interpreter.run(compiled.byte_code).await.unwrap();

            assert_eq!(result.get_val().unwrap(), TypeAnnotatedValue::U64(0));
        }

        #[test]
        async fn test_pattern_match_on_tuple() {
            let mut interpreter = Interpreter::default();

            let expr = r#"
           let x: tuple<u64, str, str> = (1, "foo", "bar");

           match x {
              (x, y, z) => "${x} ${y} ${z}"
           }
        "#;

            let mut expr = Expr::from_text(expr).unwrap();
            expr.infer_types(&FunctionTypeRegistry::empty()).unwrap();
            let compiled = compiler::compile(&expr, &vec![]).unwrap();
            let result = interpreter.run(compiled.byte_code).await.unwrap();

            assert_eq!(
                result.get_val().unwrap(),
                TypeAnnotatedValue::Str("1 foo bar".to_string())
            );
        }

        #[test]
        async fn test_pattern_match_on_tuple_with_option_some() {
            let mut interpreter = Interpreter::default();

            let expr = r#"
           let x: tuple<u64, option<str>, str> = (1, some("foo"), "bar");

           match x {
              (x, none, z) => "${x} ${z}",
              (x, some(y), z) => "${x} ${y} ${z}"
           }
        "#;

            let mut expr = Expr::from_text(expr).unwrap();
            expr.infer_types(&FunctionTypeRegistry::empty()).unwrap();

            let compiled = compiler::compile(&expr, &vec![]).unwrap();
            let result = interpreter.run(compiled.byte_code).await.unwrap();

            assert_eq!(
                result.get_val().unwrap(),
                TypeAnnotatedValue::Str("1 foo bar".to_string())
            );
        }

        #[test]
        async fn test_pattern_match_on_tuple_with_option_none() {
            let mut interpreter = Interpreter::default();

            let expr = r#"
           let x: tuple<u64, option<str>, str> = (1, none, "bar");

           match x {
              (x, none, z) => "${x} ${z}",
              (x, some(y), z) => "${x} ${y} ${z}"
           }
        "#;

            let expr = Expr::from_text(expr).unwrap();
            let compiled = compiler::compile(&expr, &vec![]).unwrap();
            let result = interpreter.run(compiled.byte_code).await.unwrap();

            assert_eq!(
                result.get_val().unwrap(),
                TypeAnnotatedValue::Str("1 bar".to_string())
            );
        }

        #[test]
        async fn test_pattern_match_on_tuple_with_all_types() {
            let mut interpreter = Interpreter::default();

            let tuple = internal::get_analysed_type_tuple();

            let analysed_exports =
                internal::get_component_metadata("foo", vec![tuple], Some(str()));

            let expr = r#"

           let record = { request : { path : { user : "jak" } }, y : "bar" };
           let input = (1, ok(100), "bar", record, process-user("jon"), register-user(1u64), validate, prod, dev, test);
           foo(input);
           match input {
             (n1, err(x1), txt, rec, process-user(x), register-user(n), validate, dev, prod, test) =>  "Invalid",
             (n1, ok(x2), txt, rec, process-user(x), register-user(n), validate, prod, dev, test) =>  "foo ${x2} ${n1} ${txt} ${rec.request.path.user} ${validate} ${prod} ${dev} ${test}"
           }

        "#;

            let expr = Expr::from_text(expr).unwrap();
            let compiled = compiler::compile(&expr, &analysed_exports).unwrap();
            let result = interpreter.run(compiled.byte_code).await.unwrap();

            assert_eq!(
                result.get_val().unwrap(),
                TypeAnnotatedValue::Str("foo 100 1 bar jak validate prod dev test".to_string())
            );
        }

        #[test]
        async fn test_pattern_match_on_tuple_with_wild_pattern() {
            let mut interpreter = Interpreter::default();

            let tuple = internal::get_analysed_type_tuple();

            let analysed_exports =
                internal::get_component_metadata("my-worker-function", vec![tuple], Some(str()));

            let expr = r#"

           let record = { request : { path : { user : "jak" } }, y : "baz" };
           let input = (1, ok(1), "bar", record, process-user("jon"), register-user(1u64), validate, prod, dev, test);
           my-worker-function(input);
           match input {
             (n1, ok(x), txt, rec, _, _, _, _, prod, _) =>  "prod ${n1} ${txt} ${rec.request.path.user} ${rec.y}",
             (n1, ok(x), txt, rec, _, _, _, _, dev, _) =>   "dev ${n1} ${txt} ${rec.request.path.user} ${rec.y}"
           }
        "#;

            let expr = Expr::from_text(expr).unwrap();
            let compiled = compiler::compile(&expr, &analysed_exports).unwrap();
            let result = interpreter.run(compiled.byte_code).await.unwrap();

            assert_eq!(
                result.get_val().unwrap(),
                TypeAnnotatedValue::Str("dev 1 bar jak baz".to_string())
            );
        }

        #[test]
        async fn test_record_output_in_pattern_match() {
            let input_analysed_type = internal::get_analysed_type_record();
            let output_analysed_type = internal::get_analysed_type_result();

            let result_value =
                internal::get_type_annotated_value(&output_analysed_type, r#"ok(1)"#);

            let mut interpreter =
                internal::static_test_interpreter(&output_analysed_type, &result_value);

            let analysed_exports = internal::get_component_metadata(
                "my-worker-function",
                vec![input_analysed_type],
                Some(output_analysed_type),
            );

            let expr = r#"

           let input = { request : { path : { user : "jak" } }, y : "baz" };
           let result = my-worker-function(input);
           match result {
             ok(result) => { body: result, status: 200u16 },
             err(result) => { status: 400u16, body: 400u64 }
           }
        "#;

            let expr = Expr::from_text(expr).unwrap();
            let compiled = compiler::compile(&expr, &analysed_exports).unwrap();
            let result = interpreter.run(compiled.byte_code).await.unwrap();

            let expected = internal::get_type_annotated_value(
                &record(vec![field("body", u64()), field("status", u16())]),
                r#"{body: 1, status: 200}"#,
            );

            assert_eq!(result.get_val().unwrap(), expected);
        }

        #[test]
        async fn test_tuple_output_in_pattern_match() {
            let input_analysed_type = internal::get_analysed_type_record();
            let output_analysed_type = internal::get_analysed_type_result();

            let result_value =
                internal::get_type_annotated_value(&output_analysed_type, r#"err("failed")"#);

            let mut interpreter =
                internal::static_test_interpreter(&output_analysed_type, &result_value);

            let analysed_exports = internal::get_component_metadata(
                "my-worker-function",
                vec![input_analysed_type],
                Some(output_analysed_type),
            );

            let expr = r#"

           let input = { request : { path : { user : "jak" } }, y : "baz" };
           let result = my-worker-function(input);
           match result {
             ok(res) => ("${res}", "foo"),
             err(msg) => (msg, "bar")
           }
        "#;

            let expr = Expr::from_text(expr).unwrap();
            let compiled = compiler::compile(&expr, &analysed_exports).unwrap();
            let result = interpreter.run(compiled.byte_code).await.unwrap();

            let expected = internal::get_type_annotated_value(
                &tuple(vec![str(), str()]),
                r#"("failed", "bar")"#,
            );

            assert_eq!(result.get_val().unwrap(), expected);
        }
    }

    mod dynamic_resource_parameter_tests {
        use test_r::test;

        use crate::interpreter::rib_interpreter::interpreter_tests::internal;
        use crate::interpreter::rib_interpreter::Interpreter;
        use crate::{compiler, Expr};
        use golem_wasm_ast::analysis::analysed_type::{
            case, f32, field, list, record, str, u32, variant,
        };
        use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;

        #[test]
        async fn test_interpreter_with_indexed_resource_drop() {
            let expr = r#"
           let user_id = "user";
           golem:it/api.{cart(user_id).drop}();
           "success"
        "#;
            let expr = Expr::from_text(expr).unwrap();
            let component_metadata =
                internal::get_shopping_cart_metadata_with_cart_resource_with_parameters();

            let compiled = compiler::compile(&expr, &component_metadata).unwrap();

            let mut rib_interpreter = Interpreter::default();
            let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

            assert_eq!(
                result.get_val().unwrap(),
                TypeAnnotatedValue::Str("success".to_string())
            );
        }

        #[test]
        async fn test_interpreter_with_indexed_resource_checkout() {
            let expr = r#"
           let user_id = "foo";
           let result = golem:it/api.{cart(user_id).checkout}();
           result
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let result_type = variant(vec![
                case("error", str()),
                case("success", record(vec![field("order-id", str())])),
            ]);

            let result_value = internal::get_type_annotated_value(
                &result_type,
                r#"
          success({order-id: "foo"})
        "#,
            );

            let component_metadata =
                internal::get_shopping_cart_metadata_with_cart_resource_with_parameters();
            let compiled = compiler::compile(&expr, &component_metadata).unwrap();

            let mut rib_executor = internal::static_test_interpreter(&result_type, &result_value);
            let result = rib_executor.run(compiled.byte_code).await.unwrap();

            assert_eq!(result.get_val().unwrap(), result_value);
        }

        #[test]
        async fn test_interpreter_with_indexed_resource_get_cart_contents() {
            let expr = r#"
           let user_id = "bar";
           let result = golem:it/api.{cart(user_id).get-cart-contents}();
           result[0].product-id
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let result_type = list(record(vec![
                field("product-id", str()),
                field("name", str()),
                field("price", f32()),
                field("quantity", u32()),
            ]));

            let result_value = internal::get_type_annotated_value(
                &result_type,
                r#"
            [{product-id: "foo", name: "bar", price: 100.0, quantity: 1}, {product-id: "bar", name: "baz", price: 200.0, quantity: 2}]
        "#,
            );

            let component_metadata =
                internal::get_shopping_cart_metadata_with_cart_resource_with_parameters();
            let compiled = compiler::compile(&expr, &component_metadata).unwrap();

            let mut rib_executor = internal::static_test_interpreter(&result_type, &result_value);
            let result = rib_executor.run(compiled.byte_code).await.unwrap();

            assert_eq!(
                result.get_val().unwrap(),
                TypeAnnotatedValue::Str("foo".to_string())
            );
        }

        #[test]
        async fn test_interpreter_with_indexed_resource_update_item_quantity() {
            let expr = r#"
           let user_id = "jon";
           let product_id = "mac";
           let quantity = 1032;
           golem:it/api.{cart(user_id).update-item-quantity}(product_id, quantity);
           "successfully updated"
        "#;
            let expr = Expr::from_text(expr).unwrap();

            let component_metadata =
                internal::get_shopping_cart_metadata_with_cart_resource_with_parameters();

            let compiled = compiler::compile(&expr, &component_metadata).unwrap();

            let mut rib_executor = Interpreter::default();

            let result = rib_executor.run(compiled.byte_code).await.unwrap();

            assert_eq!(
                result.get_val().unwrap(),
                TypeAnnotatedValue::Str("successfully updated".to_string())
            );
        }

        #[test]
        async fn test_interpreter_with_indexed_resource_add_item() {
            let expr = r#"
           let user_id = "foo";
           let product = { product-id: "mac", name: "macbook", quantity: 1u32, price: 1f32 };
           golem:it/api.{cart(user_id).add-item}(product);

           "successfully added"
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let component_metadata =
                internal::get_shopping_cart_metadata_with_cart_resource_with_parameters();

            let compiled = compiler::compile(&expr, &component_metadata).unwrap();

            let mut rib_executor = Interpreter::default();

            let result = rib_executor.run(compiled.byte_code).await.unwrap();

            assert_eq!(
                result.get_val().unwrap(),
                TypeAnnotatedValue::Str("successfully added".to_string())
            );
        }

        #[test]
        async fn test_interpreter_with_resource_add_item() {
            let expr = r#"
           let user_id = "foo";
           let product = { product-id: "mac", name: "macbook", quantity: 1u32, price: 1f32 };
           golem:it/api.{cart.add-item}(product);

           "successfully added"
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let component_metadata = internal::get_shopping_cart_metadata_with_cart_raw_resource();

            let compiled = compiler::compile(&expr, &component_metadata).unwrap();

            let mut rib_executor = Interpreter::default();

            let result = rib_executor.run(compiled.byte_code).await.unwrap();

            assert_eq!(
                result.get_val().unwrap(),
                TypeAnnotatedValue::Str("successfully added".to_string())
            );
        }

        #[test]
        async fn test_interpreter_with_resource_get_cart_contents() {
            let expr = r#"
           let result = golem:it/api.{cart.get-cart-contents}();
           result[0].product-id
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let result_type = list(record(vec![
                field("product-id", str()),
                field("name", str()),
                field("price", f32()),
                field("quantity", u32()),
            ]));

            let result_value = internal::get_type_annotated_value(
                &result_type,
                r#"
            [{product-id: "foo", name: "bar", price: 100.0, quantity: 1}, {product-id: "bar", name: "baz", price: 200.0, quantity: 2}]
        "#,
            );

            let component_metadata = internal::get_shopping_cart_metadata_with_cart_raw_resource();
            let compiled = compiler::compile(&expr, &component_metadata).unwrap();

            let mut rib_executor = internal::static_test_interpreter(&result_type, &result_value);
            let result = rib_executor.run(compiled.byte_code).await.unwrap();

            assert_eq!(
                result.get_val().unwrap(),
                TypeAnnotatedValue::Str("foo".to_string())
            );
        }

        #[test]
        async fn test_interpreter_with_resource_update_item() {
            let expr = r#"
           let product_id = "mac";
           let quantity = 1032;
           golem:it/api.{cart.update-item-quantity}(product_id, quantity);
           "successfully updated"
        "#;
            let expr = Expr::from_text(expr).unwrap();

            let component_metadata = internal::get_shopping_cart_metadata_with_cart_raw_resource();

            let compiled = compiler::compile(&expr, &component_metadata).unwrap();

            let mut rib_executor = Interpreter::default();

            let result = rib_executor.run(compiled.byte_code).await.unwrap();

            assert_eq!(
                result.get_val().unwrap(),
                TypeAnnotatedValue::Str("successfully updated".to_string())
            );
        }

        #[test]
        async fn test_interpreter_with_resource_checkout() {
            let expr = r#"
           let result = golem:it/api.{cart.checkout}();
           result
        "#;

            let expr = Expr::from_text(expr).unwrap();

            let result_type = variant(vec![
                case("error", str()),
                case("success", record(vec![field("order-id", str())])),
            ]);

            let result_value = internal::get_type_annotated_value(
                &result_type,
                r#"
          success({order-id: "foo"})
        "#,
            );

            let component_metadata = internal::get_shopping_cart_metadata_with_cart_raw_resource();
            let compiled = compiler::compile(&expr, &component_metadata).unwrap();

            let mut rib_executor = internal::static_test_interpreter(&result_type, &result_value);
            let result = rib_executor.run(compiled.byte_code).await.unwrap();

            assert_eq!(result.get_val().unwrap(), result_value);
        }

        #[test]
        async fn test_interpreter_with_resource_drop() {
            let expr = r#"
           golem:it/api.{cart.drop}();
           "success"
        "#;
            let expr = Expr::from_text(expr).unwrap();
            let component_metadata = internal::get_shopping_cart_metadata_with_cart_raw_resource();

            let compiled = compiler::compile(&expr, &component_metadata).unwrap();

            let mut rib_interpreter = Interpreter::default();
            let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

            assert_eq!(
                result.get_val().unwrap(),
                TypeAnnotatedValue::Str("success".to_string())
            );
        }
    }

    mod internal {
        use crate::interpreter::rib_interpreter::Interpreter;
        use crate::{RibFunctionInvoke, RibInput};
        use golem_wasm_ast::analysis::analysed_type::{
            case, f32, field, handle, list, r#enum, record, result, str, tuple, u32, u64,
            unit_case, variant,
        };
        use golem_wasm_ast::analysis::{
            AnalysedExport, AnalysedFunction, AnalysedFunctionParameter, AnalysedFunctionResult,
            AnalysedInstance, AnalysedResourceId, AnalysedResourceMode, AnalysedType,
        };
        use golem_wasm_rpc::protobuf::type_annotated_value::TypeAnnotatedValue;
        use golem_wasm_rpc::protobuf::TypedTuple;
        use std::sync::Arc;

        pub(crate) fn get_analysed_type_variant() -> AnalysedType {
            variant(vec![
                case("register-user", u64()),
                case("process-user", str()),
                unit_case("validate"),
            ])
        }

        pub(crate) fn get_analysed_type_record() -> AnalysedType {
            record(vec![
                field(
                    "request",
                    record(vec![field("path", record(vec![field("user", str())]))]),
                ),
                field("y", str()),
            ])
        }

        pub(crate) fn get_analysed_type_result() -> AnalysedType {
            result(u64(), str())
        }

        pub(crate) fn get_analysed_type_enum() -> AnalysedType {
            r#enum(&["prod", "dev", "test"])
        }

        pub(crate) fn get_analysed_typ_str() -> AnalysedType {
            str()
        }

        pub(crate) fn get_analysed_typ_u64() -> AnalysedType {
            u64()
        }

        pub(crate) fn get_analysed_type_tuple() -> AnalysedType {
            tuple(vec![
                get_analysed_typ_u64(),
                get_analysed_type_result(),
                get_analysed_typ_str(),
                get_analysed_type_record(),
                get_analysed_type_variant(),
                get_analysed_type_variant(),
                get_analysed_type_variant(),
                get_analysed_type_enum(),
                get_analysed_type_enum(),
                get_analysed_type_enum(),
            ])
        }

        pub(crate) fn get_component_metadata(
            function_name: &str,
            input_types: Vec<AnalysedType>,
            output: Option<AnalysedType>,
        ) -> Vec<AnalysedExport> {
            let analysed_function_parameters = input_types
                .into_iter()
                .enumerate()
                .map(|(index, typ)| AnalysedFunctionParameter {
                    name: format!("param{}", index),
                    typ,
                })
                .collect();

            let results = if let Some(output) = output {
                vec![AnalysedFunctionResult {
                    name: None,
                    typ: output,
                }]
            } else {
                // Representing Unit
                vec![]
            };

            vec![AnalysedExport::Function(AnalysedFunction {
                name: function_name.to_string(),
                parameters: analysed_function_parameters,
                results,
            })]
        }

        pub(crate) fn get_shopping_cart_metadata_with_cart_resource_with_parameters(
        ) -> Vec<AnalysedExport> {
            get_shopping_cart_metadata_with_cart_resource(vec![AnalysedFunctionParameter {
                name: "user-id".to_string(),
                typ: str(),
            }])
        }

        pub(crate) fn get_shopping_cart_metadata_with_cart_raw_resource() -> Vec<AnalysedExport> {
            get_shopping_cart_metadata_with_cart_resource(vec![])
        }

        fn get_shopping_cart_metadata_with_cart_resource(
            constructor_parameters: Vec<AnalysedFunctionParameter>,
        ) -> Vec<AnalysedExport> {
            let instance = AnalysedExport::Instance(AnalysedInstance {
                name: "golem:it/api".to_string(),
                functions: vec![
                    AnalysedFunction {
                        name: "[constructor]cart".to_string(),
                        parameters: constructor_parameters,
                        results: vec![AnalysedFunctionResult {
                            name: None,
                            typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Owned),
                        }],
                    },
                    AnalysedFunction {
                        name: "[method]cart.add-item".to_string(),
                        parameters: vec![
                            AnalysedFunctionParameter {
                                name: "self".to_string(),
                                typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                            },
                            AnalysedFunctionParameter {
                                name: "item".to_string(),
                                typ: record(vec![
                                    field("product-id", str()),
                                    field("name", str()),
                                    field("price", f32()),
                                    field("quantity", u32()),
                                ]),
                            },
                        ],
                        results: vec![],
                    },
                    AnalysedFunction {
                        name: "[method]cart.remove-item".to_string(),
                        parameters: vec![
                            AnalysedFunctionParameter {
                                name: "self".to_string(),
                                typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                            },
                            AnalysedFunctionParameter {
                                name: "product-id".to_string(),
                                typ: str(),
                            },
                        ],
                        results: vec![],
                    },
                    AnalysedFunction {
                        name: "[method]cart.update-item-quantity".to_string(),
                        parameters: vec![
                            AnalysedFunctionParameter {
                                name: "self".to_string(),
                                typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                            },
                            AnalysedFunctionParameter {
                                name: "product-id".to_string(),
                                typ: str(),
                            },
                            AnalysedFunctionParameter {
                                name: "quantity".to_string(),
                                typ: u32(),
                            },
                        ],
                        results: vec![],
                    },
                    AnalysedFunction {
                        name: "[method]cart.checkout".to_string(),
                        parameters: vec![AnalysedFunctionParameter {
                            name: "self".to_string(),
                            typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                        }],
                        results: vec![AnalysedFunctionResult {
                            name: None,
                            typ: variant(vec![
                                case("error", str()),
                                case("success", record(vec![field("order-id", str())])),
                            ]),
                        }],
                    },
                    AnalysedFunction {
                        name: "[method]cart.get-cart-contents".to_string(),
                        parameters: vec![AnalysedFunctionParameter {
                            name: "self".to_string(),
                            typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                        }],
                        results: vec![AnalysedFunctionResult {
                            name: None,
                            typ: list(record(vec![
                                field("product-id", str()),
                                field("name", str()),
                                field("price", f32()),
                                field("quantity", u32()),
                            ])),
                        }],
                    },
                    AnalysedFunction {
                        name: "[method]cart.merge-with".to_string(),
                        parameters: vec![
                            AnalysedFunctionParameter {
                                name: "self".to_string(),
                                typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                            },
                            AnalysedFunctionParameter {
                                name: "other-cart".to_string(),
                                typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                            },
                        ],
                        results: vec![],
                    },
                    AnalysedFunction {
                        name: "[drop]cart".to_string(),
                        parameters: vec![AnalysedFunctionParameter {
                            name: "self".to_string(),
                            typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Owned),
                        }],
                        results: vec![],
                    },
                ],
            });

            vec![instance]
        }

        pub(crate) fn get_type_annotated_value(
            analysed_type: &AnalysedType,
            wasm_wave_str: &str,
        ) -> TypeAnnotatedValue {
            golem_wasm_rpc::type_annotated_value_from_str(analysed_type, wasm_wave_str).unwrap()
        }

        pub(crate) fn static_test_interpreter(
            result_type: &AnalysedType,
            result_value: &TypeAnnotatedValue,
        ) -> Interpreter {
            Interpreter {
                input: RibInput::default(),
                invoke: static_worker_invoke(result_type, result_value),
            }
        }

        fn static_worker_invoke(
            result_type: &AnalysedType,
            value: &TypeAnnotatedValue,
        ) -> RibFunctionInvoke {
            let analysed_type = result_type.clone();
            let value = value.clone();

            Arc::new(move |_, _| {
                Box::pin({
                    let analysed_type = analysed_type.clone();
                    let value = value.clone();

                    async move {
                        let analysed_type = analysed_type.clone();
                        let value = value.clone();
                        Ok(TypeAnnotatedValue::Tuple(TypedTuple {
                            typ: vec![golem_wasm_ast::analysis::protobuf::Type::from(
                                &analysed_type,
                            )],
                            value: vec![golem_wasm_rpc::protobuf::TypeAnnotatedValue {
                                type_annotated_value: Some(value.clone()),
                            }],
                        }))
                    }
                })
            })
        }
    }
}
