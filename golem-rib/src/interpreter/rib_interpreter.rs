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

use crate::interpreter::env::InterpreterEnv;
use crate::interpreter::instruction_cursor::RibByteCodeCursor;
use crate::interpreter::stack::InterpreterStack;
use crate::{RibByteCode, RibFunctionInvoke, RibIR, RibInput, RibResult};
use std::sync::Arc;

use super::interpreter_stack_value::RibInterpreterStackValue;

pub struct Interpreter {
    pub input: RibInput,
    pub invoke: Arc<dyn RibFunctionInvoke + Sync + Send>,
    pub custom_stack: Option<InterpreterStack>,
    pub custom_env: Option<InterpreterEnv>,
}

impl Default for Interpreter {
    fn default() -> Self {
        Interpreter {
            input: RibInput::default(),
            invoke: Arc::new(internal::NoopRibFunctionInvoke),
            custom_stack: None,
            custom_env: None,
        }
    }
}

impl Interpreter {
    pub fn new(
        input: &RibInput,
        invoke: Arc<dyn RibFunctionInvoke + Sync + Send>,
        custom_stack: Option<InterpreterStack>,
        custom_env: Option<InterpreterEnv>,
    ) -> Self {
        Interpreter {
            input: input.clone(),
            invoke,
            custom_stack,
            custom_env,
        }
    }

    // Interpreter that's not expected to call a side-effecting function call.
    // All it needs is environment with the required variables to evaluate the Rib script
    pub fn pure(
        input: &RibInput,
        custom_stack: Option<InterpreterStack>,
        custom_env: Option<InterpreterEnv>,
    ) -> Self {
        Interpreter {
            input: input.clone(),
            invoke: Arc::new(internal::NoopRibFunctionInvoke),
            custom_stack,
            custom_env,
        }
    }

    // Override rib input helps with incremental interpretation
    // where a rib script is now trying to access a specific input
    // such that the compiler already knows the required global variable and its types,
    // to later override the interpreter with this input. The previous inputs will be completely
    // discard as they are either loaded in as variables, or if they are accessed again, the inputs
    // will be or can be overriden back
    pub fn override_rib_input(&mut self, rib_input: RibInput) {
        self.input = rib_input;
    }

    pub async fn run(&mut self, instructions0: RibByteCode) -> Result<RibResult, String> {
        let mut byte_code_cursor = RibByteCodeCursor::from_rib_byte_code(instructions0);
        let stack = match &mut self.custom_stack {
            Some(custom) => custom,
            None => &mut InterpreterStack::default(),
        };

        let interpreter_env = match &mut self.custom_env {
            Some(custom) => custom,
            None => &mut InterpreterEnv::from(&self.input, &self.invoke),
        };

        while let Some(instruction) = byte_code_cursor.get_instruction() {
            match instruction {
                RibIR::PushLit(val) => {
                    stack.push_val(val);
                }

                RibIR::PushFlag(val) => {
                    stack.push_val(val);
                }

                RibIR::CreateAndPushRecord(analysed_type) => {
                    internal::run_create_record_instruction(analysed_type, stack)?;
                }

                RibIR::UpdateRecord(field_name) => {
                    internal::run_update_record_instruction(field_name, stack)?;
                }

                RibIR::PushList(analysed_type, arg_size) => {
                    internal::run_push_list_instruction(arg_size, analysed_type, stack)?;
                }

                RibIR::EqualTo => {
                    internal::run_compare_instruction(stack, |left, right| left == right)?;
                }

                RibIR::GreaterThan => {
                    internal::run_compare_instruction(stack, |left, right| left > right)?;
                }

                RibIR::LessThan => {
                    internal::run_compare_instruction(stack, |left, right| left < right)?;
                }

                RibIR::GreaterThanOrEqualTo => {
                    internal::run_compare_instruction(stack, |left, right| left >= right)?;
                }

                RibIR::LessThanOrEqualTo => {
                    internal::run_compare_instruction(stack, |left, right| left <= right)?;
                }
                RibIR::Plus(analysed_type) => {
                    internal::run_math_instruction(
                        stack,
                        |left, right| left + right,
                        &analysed_type,
                    )?;
                }
                RibIR::Minus(analysed_type) => {
                    internal::run_math_instruction(
                        stack,
                        |left, right| left - right,
                        &analysed_type,
                    )?;
                }
                RibIR::Divide(analysed_type) => {
                    internal::run_math_instruction(
                        stack,
                        |left, right| left / right,
                        &analysed_type,
                    )?;
                }
                RibIR::Multiply(analysed_type) => {
                    internal::run_math_instruction(
                        stack,
                        |left, right| left * right,
                        &analysed_type,
                    )?;
                }

                RibIR::AssignVar(variable_id) => {
                    internal::run_assign_var_instruction(variable_id, stack, interpreter_env)?;
                }

                RibIR::LoadVar(variable_id) => {
                    internal::run_load_var_instruction(variable_id, stack, interpreter_env)?;
                }

                RibIR::IsEmpty => {
                    internal::run_is_empty_instruction(stack)?;
                }

                RibIR::JumpIfFalse(instruction_id) => {
                    internal::run_jump_if_false_instruction(
                        instruction_id,
                        &mut byte_code_cursor,
                        stack,
                    )?;
                }

                RibIR::SelectField(field_name) => {
                    internal::run_select_field_instruction(field_name, stack)?;
                }

                RibIR::SelectIndex(index) => {
                    internal::run_select_index_instruction(stack, index)?;
                }

                RibIR::SelectIndexV1 => {
                    internal::run_select_index_v1_instruction(stack)?;
                }

                RibIR::CreateFunctionName(site, function_type) => {
                    internal::run_create_function_name_instruction(site, function_type, stack)?;
                }

                RibIR::InvokeFunction(worker_type, arg_size, _) => {
                    internal::run_call_instruction(arg_size, worker_type, stack, interpreter_env)
                        .await?;
                }

                RibIR::PushVariant(variant_name, analysed_type) => {
                    internal::run_variant_construction_instruction(
                        variant_name,
                        analysed_type,
                        stack,
                    )
                    .await?;
                }

                RibIR::PushEnum(enum_name, analysed_type) => {
                    internal::run_push_enum_instruction(stack, enum_name, analysed_type)?;
                }

                RibIR::Throw(message) => {
                    return Err(message);
                }

                RibIR::GetTag => {
                    internal::run_get_tag_instruction(stack)?;
                }

                RibIR::Deconstruct => {
                    internal::run_deconstruct_instruction(stack)?;
                }

                RibIR::Jump(instruction_id) => {
                    byte_code_cursor.move_to(&instruction_id).ok_or_else(|| {
                        format!(
                            "internal error. Failed to move to label {}",
                            instruction_id.index
                        )
                    })?;
                }

                RibIR::PushSome(analysed_type) => {
                    internal::run_create_some_instruction(stack, analysed_type)?;
                }
                RibIR::PushNone(analysed_type) => {
                    internal::run_create_none_instruction(stack, analysed_type)?;
                }
                RibIR::PushOkResult(analysed_type) => {
                    internal::run_create_ok_instruction(stack, analysed_type)?;
                }
                RibIR::PushErrResult(analysed_type) => {
                    internal::run_create_err_instruction(stack, analysed_type)?;
                }
                RibIR::Concat(arg_size) => {
                    internal::run_concat_instruction(stack, arg_size)?;
                }
                RibIR::PushTuple(analysed_type, arg_size) => {
                    internal::run_push_tuple_instruction(arg_size, analysed_type, stack)?;
                }
                RibIR::Negate => {
                    internal::run_negate_instruction(stack)?;
                }

                RibIR::Label(_) => {}

                RibIR::And => {
                    internal::run_and_instruction(stack)?;
                }

                RibIR::Or => {
                    internal::run_or_instruction(stack)?;
                }
                RibIR::ToIterator => {
                    internal::run_to_iterator(stack)?;
                }
                RibIR::CreateSink(analysed_type) => {
                    internal::run_create_sink_instruction(stack, &analysed_type)?
                }
                RibIR::AdvanceIterator => {
                    internal::run_advance_iterator_instruction(stack)?;
                }
                RibIR::PushToSink => {
                    internal::run_push_to_sink_instruction(stack)?;
                }

                RibIR::SinkToList => {
                    internal::run_sink_to_list_instruction(stack)?;
                }

                RibIR::Length => {
                    internal::run_length_instruction(stack)?;
                }
            }
        }

        let stack_value = stack
            .pop()
            .unwrap_or_else(|| RibInterpreterStackValue::Unit);

        let rib_result = RibResult::from_rib_interpreter_stack_value(&stack_value)
            .ok_or_else(|| "Failed to obtain a valid result from rib execution".to_string())?;

        Ok(rib_result)
    }
}

mod internal {
    use crate::interpreter::env::{EnvironmentKey, InterpreterEnv};
    use crate::interpreter::interpreter_stack_value::RibInterpreterStackValue;
    use crate::interpreter::literal::LiteralValue;
    use crate::interpreter::stack::InterpreterStack;
    use crate::{
        CoercedNumericValue, EvaluatedFnArgs, EvaluatedFqFn, EvaluatedWorkerName,
        FunctionReferenceType, InstructionId, ParsedFunctionName, ParsedFunctionReference,
        ParsedFunctionSite, RibFunctionInvoke, VariableId, WorkerNamePresence,
    };
    use golem_wasm_ast::analysis::AnalysedType;
    use golem_wasm_ast::analysis::TypeResult;
    use golem_wasm_rpc::{print_value_and_type, IntoValueAndType, Value, ValueAndType};

    use crate::interpreter::instruction_cursor::RibByteCodeCursor;
    use async_trait::async_trait;
    use golem_wasm_ast::analysis::analysed_type::{tuple, u64};
    use std::ops::Deref;

    pub(crate) struct NoopRibFunctionInvoke;

    #[async_trait]
    impl RibFunctionInvoke for NoopRibFunctionInvoke {
        async fn invoke(
            &self,
            _worker_name: Option<EvaluatedWorkerName>,
            _function_name: EvaluatedFqFn,
            _args: EvaluatedFnArgs,
        ) -> Result<ValueAndType, String> {
            Ok(ValueAndType {
                value: Value::Tuple(vec![]),
                typ: tuple(vec![]),
            })
        }
    }

    pub(crate) fn run_is_empty_instruction(
        interpreter_stack: &mut InterpreterStack,
    ) -> Result<(), String> {
        let rib_result = interpreter_stack.pop().ok_or_else(|| {
            "internal Error: Failed to get a value from the stack to do check is_empty".to_string()
        })?;

        let bool_opt = match rib_result {
            RibInterpreterStackValue::Val(ValueAndType {
                value: Value::List(items),
                ..
            }) => Some(items.is_empty()),
            RibInterpreterStackValue::Iterator(iter) => {
                let mut peekable_iter = iter.peekable();
                let result = peekable_iter.peek().is_some();
                interpreter_stack.push(RibInterpreterStackValue::Iterator(Box::new(peekable_iter)));
                Some(result)
            }
            RibInterpreterStackValue::Sink(values, analysed_type) => {
                let possible_iterator = interpreter_stack.pop().ok_or_else(|| {
                    "internal error: Expecting an iterator to check is empty".to_string()
                })?;

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

        let bool = bool_opt.ok_or("internal error: Failed to run instruction is_empty")?;
        interpreter_stack.push_val(bool.into_value_and_type());
        Ok(())
    }

    pub(crate) fn run_jump_if_false_instruction(
        instruction_id: InstructionId,
        instruction_stack: &mut RibByteCodeCursor,
        interpreter_stack: &mut InterpreterStack,
    ) -> Result<(), String> {
        let predicate = interpreter_stack.try_pop_bool()?;

        // Jump if predicate is false
        if !predicate {
            instruction_stack.move_to(&instruction_id).ok_or_else(|| {
                format!(
                    "internal error: Failed to move to the instruction at {}",
                    instruction_id.index
                )
            })?;
        }

        Ok(())
    }

    pub(crate) fn run_to_iterator(interpreter_stack: &mut InterpreterStack) -> Result<(), String> {
        let popped_up = interpreter_stack
            .pop()
            .ok_or_else(|| "internal error: failed to get a value from the stack".to_string())?;

        let value_and_type = popped_up
            .get_val()
            .ok_or_else(|| "internal error: failed to get a value from the stack".to_string())?;

        match (value_and_type.value, value_and_type.typ) {
            (Value::List(items), AnalysedType::List(item_type)) => {
                let items = items
                    .into_iter()
                    .map(|item| ValueAndType::new(item, (*item_type.inner).clone()))
                    .collect::<Vec<_>>();

                interpreter_stack.push(RibInterpreterStackValue::Iterator(Box::new(
                    items.into_iter(),
                )));

                Ok(())
            }
            (Value::Record(fields), AnalysedType::Record(record_type)) => {
                let mut from: Option<usize> = None;
                let mut to: Option<usize> = None;
                let mut inclusive = false;

                let value_and_names = fields.into_iter().zip(record_type.fields);

                for (value, name_and_type) in value_and_names {
                    match name_and_type.name.as_str() {
                        "from" => {
                            from =
                                Some(to_num(&value).ok_or_else(|| {
                                    format!("cannot cast {:?} to a number", value)
                                })?)
                        }
                        "to" => {
                            to =
                                Some(to_num(&value).ok_or_else(|| {
                                    format!("cannot cast {:?} to a number", value)
                                })?)
                        }
                        "inclusive" => {
                            inclusive = match value {
                                Value::Bool(b) => b,
                                _ => return Err("inclusive field should be a boolean".to_string()),
                            }
                        }
                        _ => return Err(format!("Invalid field name {}", name_and_type.name)),
                    }
                }

                match (from, to) {
                    (Some(from), Some(to)) => {
                        if inclusive {
                            interpreter_stack.push(RibInterpreterStackValue::Iterator(Box::new(
                                (from..=to)
                                    .map(|i| ValueAndType::new(Value::U64(i as u64), u64())),
                            )));
                        } else {
                            interpreter_stack.push(RibInterpreterStackValue::Iterator(Box::new(
                                (from..to)
                                    .map(|i| ValueAndType::new(Value::U64(i as u64), u64())),
                            )));
                        }
                    }

                    (None, Some(to)) => {
                        if inclusive {
                            interpreter_stack.push(RibInterpreterStackValue::Iterator(Box::new(
                                (0..=to)
                                    .map(|i| ValueAndType::new(Value::U64(i as u64), u64())),
                            )));
                        } else {
                            interpreter_stack.push(RibInterpreterStackValue::Iterator(Box::new(
                                (0..to)
                                    .map(|i| ValueAndType::new(Value::U64(i as u64), u64())),
                            )));
                        }
                    }

                    // avoiding panicking with stack overflow for rib like the following
                    // for i in 0.. {
                    //   yield i
                    // }
                    (Some(_), None) => {
                        return Err("an infinite range is being iterated. make sure range is finite to avoid infinite computation".to_string())
                    }

                    (None, None) => {
                        interpreter_stack.push(RibInterpreterStackValue::Iterator(Box::new({
                            let range = 0..;
                            range
                                .into_iter()
                                .map(|i| ValueAndType::new(Value::U64(i as u64), u64()))
                        })));
                    }
                };

                Ok(())
            }

            _ => Err("internal error: failed to convert to an iterator".to_string()),
        }
    }

    fn to_num(value: &Value) -> Option<usize> {
        match value {
            Value::U64(u64) => Some(*u64 as usize),
            Value::Bool(_) => None,
            Value::U8(u8) => Some(*u8 as usize),
            Value::U16(u16) => Some(*u16 as usize),
            Value::U32(u32) => Some(*u32 as usize),
            Value::S8(s8) => Some(*s8 as usize),
            Value::S16(s16) => Some(*s16 as usize),
            Value::S32(s32) => Some(*s32 as usize),
            Value::S64(s64) => Some(*s64 as usize),
            Value::F32(f32) => Some(*f32 as usize),
            Value::F64(f64) => Some(*f64 as usize),
            _ => None,
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
            .ok_or_else(|| "internal error: failed to advance the iterator".to_string())?;

        match &mut rib_result {
            RibInterpreterStackValue::Sink(_, _) => {
                let mut existing_iterator = interpreter_stack
                    .pop()
                    .ok_or("internal error: failed to get an iterator")?;

                match &mut existing_iterator {
                    RibInterpreterStackValue::Iterator(iter) => {
                        if let Some(value_and_type) = iter.next() {
                            interpreter_stack.push(existing_iterator);
                            interpreter_stack.push(rib_result);
                            interpreter_stack.push(RibInterpreterStackValue::Val(value_and_type));
                            Ok(())
                        } else {
                            Err("no more items found in the iterator".to_string())
                        }
                    }

                    _ => Err(
                        "internal error: A sink cannot exist without a corresponding iterator"
                            .to_string(),
                    ),
                }
            }

            RibInterpreterStackValue::Iterator(iter) => {
                if let Some(value_and_type) = iter.next() {
                    interpreter_stack.push(rib_result);
                    interpreter_stack.push(RibInterpreterStackValue::Val(value_and_type));
                    Ok(())
                } else {
                    Err("no more items found in the iterator".to_string())
                }
            }
            _ => Err("internal Error: expected an iterator".to_string()),
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
        let (result, analysed_type) = interpreter_stack
            .pop_sink()
            .ok_or("Failed to retrieve items from sink")?;

        interpreter_stack.push_list(
            result.into_iter().map(|vnt| vnt.value).collect(),
            &analysed_type,
        );

        Ok(())
    }

    pub(crate) fn run_length_instruction(
        interpreter_stack: &mut InterpreterStack,
    ) -> Result<(), String> {
        let rib_result = interpreter_stack
            .pop()
            .ok_or("internal error: failed to get a value from the stack")?;

        let length = match rib_result {
            RibInterpreterStackValue::Val(ValueAndType {
                value: Value::List(items),
                ..
            }) => items.len(),
            RibInterpreterStackValue::Iterator(iter) => iter.count(),
            _ => return Err("internal error: failed to get the length of the value".to_string()),
        };

        interpreter_stack.push_val(ValueAndType::new(Value::U64(length as u64), u64()));
        Ok(())
    }

    pub(crate) fn run_assign_var_instruction(
        variable_id: VariableId,
        interpreter_stack: &mut InterpreterStack,
        interpreter_env: &mut InterpreterEnv,
    ) -> Result<(), String> {
        let value = interpreter_stack.pop().ok_or_else(|| {
            "Expected a value on the stack before assigning a variable".to_string()
        })?;
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
        let value = interpreter_env.lookup(&env_key).ok_or_else(|| {
            format!(
                "`{}` not found. If this is a global input, pass it to the rib interpreter",
                variable_id
            )
        })?;

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
            AnalysedType::Record(type_record) => type_record.fields,
            _ => {
                return Err(format!(
                    "internal error: expected a record type to create a record, but obtained {:?}",
                    analysed_type
                ))
            }
        };

        interpreter_stack.create_record(name_type_pair);
        Ok(())
    }

    pub(crate) fn run_update_record_instruction(
        field_name: String,
        interpreter_stack: &mut InterpreterStack,
    ) -> Result<(), String> {
        let (current_record_fields, record_type) = interpreter_stack.try_pop_record()?;

        let idx = record_type
            .fields
            .iter()
            .position(|pair| pair.name == field_name)
            .ok_or_else(|| {
                format!(
                    "Invalid field name {field_name}, should be one of {}",
                    record_type
                        .fields
                        .iter()
                        .map(|pair| pair.name.clone())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
        let value = interpreter_stack.try_pop_val()?;

        let mut fields = current_record_fields;
        fields[idx] = value.value;

        interpreter_stack.push_val(ValueAndType {
            value: Value::Record(fields),
            typ: AnalysedType::Record(record_type),
        });
        Ok(())
    }

    pub(crate) fn run_push_list_instruction(
        list_size: usize,
        analysed_type: AnalysedType,
        interpreter_stack: &mut InterpreterStack,
    ) -> Result<(), String> {
        match analysed_type {
            AnalysedType::List(inner_type) => {
                let items =
                    interpreter_stack.try_pop_n_val(list_size)?;


                interpreter_stack.push_list(items.into_iter().map(|vnt| vnt.value).collect(), inner_type.inner.deref());

                Ok(())
            }

            _ => Err(format!("internal error: failed to create tuple due to mismatch in types. expected: list, actual: {:?}", analysed_type)),
        }
    }

    pub(crate) fn run_push_tuple_instruction(
        list_size: usize,
        analysed_type: AnalysedType,
        interpreter_stack: &mut InterpreterStack,
    ) -> Result<(), String> {
        match analysed_type {
            AnalysedType::Tuple(_inner_type) => {
                let items =
                    interpreter_stack.try_pop_n_val(list_size)?;
                interpreter_stack.push_tuple(items);
                Ok(())
            }

            _ => Err(format!("internal error: failed to create tuple due to mismatch in types. expected: tuple, actual: {:?}", analysed_type)),
        }
    }

    pub(crate) fn run_negate_instruction(
        interpreter_stack: &mut InterpreterStack,
    ) -> Result<(), String> {
        let bool = interpreter_stack.try_pop_bool()?;
        let negated = !bool;

        interpreter_stack.push_val(negated.into_value_and_type());
        Ok(())
    }

    pub(crate) fn run_and_instruction(
        interpreter_stack: &mut InterpreterStack,
    ) -> Result<(), String> {
        let left = interpreter_stack.try_pop()?;
        let right = interpreter_stack.try_pop()?;

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
        let left = interpreter_stack.try_pop()?;
        let right = interpreter_stack.try_pop()?;

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
        let left = interpreter_stack.try_pop()?;
        let right = interpreter_stack.try_pop()?;

        let result = left.evaluate_math_op(&right, compare_fn)?;
        let numerical_type = result.cast_to(target_numerical_type).ok_or_else(|| {
            format!(
                "failed to cast number {} to {:?}",
                result, target_numerical_type
            )
        })?;

        interpreter_stack.push_val(numerical_type);

        Ok(())
    }

    pub(crate) fn run_compare_instruction(
        interpreter_stack: &mut InterpreterStack,
        compare_fn: fn(LiteralValue, LiteralValue) -> bool,
    ) -> Result<(), String> {
        let left = interpreter_stack.try_pop()?;
        let right = interpreter_stack.try_pop()?;

        let result = left.compare(&right, compare_fn)?;

        interpreter_stack.push(result);

        Ok(())
    }

    // Kept for backward compatibility with byte code
    pub(crate) fn run_select_field_instruction(
        field_name: String,
        interpreter_stack: &mut InterpreterStack,
    ) -> Result<(), String> {
        let record = interpreter_stack.try_pop()?;

        match record {
            RibInterpreterStackValue::Val(ValueAndType {
                value: Value::Record(field_values),
                typ: AnalysedType::Record(typ),
            }) => {
                let field = field_values
                    .into_iter()
                    .zip(typ.fields)
                    .find(|(_value, field)| field.name == field_name)
                    .ok_or_else(|| format!("Field {} not found in the record", field_name))?;

                let value = field.0;
                interpreter_stack.push_val(ValueAndType::new(value, field.1.typ));
                Ok(())
            }
            result => {
                let stack_value_as_string = String::try_from(result)?;

                Err(format!(
                    "Unable to select field `{}` as the input `{}` is not a `record` type",
                    field_name, stack_value_as_string
                ))
            }
        }
    }

    pub(crate) fn run_select_index_v1_instruction(
        interpreter_stack: &mut InterpreterStack,
    ) -> Result<(), String> {
        let stack_list_value = interpreter_stack
            .pop()
            .ok_or_else(|| "internal error: failed to get value from the stack".to_string())?;

        let index_value = interpreter_stack
            .pop()
            .ok_or("internal error: failed to get the index expression from the stack")?;

        match stack_list_value {
            RibInterpreterStackValue::Val(ValueAndType {
                value: Value::List(items),
                typ: AnalysedType::List(typ),
            }) => match index_value.get_literal().and_then(|v| v.get_number()) {
                Some(CoercedNumericValue::PosInt(index)) => {
                    let value = items
                        .get(index as usize)
                        .ok_or_else(|| format!(
                            "index {} is out of bound in the list of length {}",
                            index,
                            items.len()
                        ))?
                        .clone();

                    interpreter_stack.push_val(ValueAndType::new(value, (*typ.inner).clone()));
                    Ok(())
                }
                _ => Err("internal error: range selection not supported at byte code level. missing desugar phase".to_string()),
            },
            RibInterpreterStackValue::Val(ValueAndType {
                value: Value::Tuple(items),
                typ: AnalysedType::Tuple(typ),
            }) => match index_value.get_literal().and_then(|v| v.get_number()) {
                Some(CoercedNumericValue::PosInt(index)) => {
                    let value = items
                        .get(index as usize)
                        .ok_or_else(|| format!(
                            "index {} is out of bound in a tuple of length {}",
                            index,
                            items.len()
                        ))?
                        .clone();

                    let item_type = typ
                        .items
                        .get(index as usize)
                        .ok_or_else(|| format!(
                            "internal error: type not found in the tuple at index {}",
                            index
                        ))?
                        .clone();

                    interpreter_stack.push_val(ValueAndType::new(value, item_type));
                    Ok(())
                }
                _ => Err("expected a number to select an index from tuple".to_string()),
            },
            result => Err(format!(
                "expected a sequence value or tuple to select an index. But obtained {:?}",
                result
            )),
        }
    }

    pub(crate) fn run_select_index_instruction(
        interpreter_stack: &mut InterpreterStack,
        index: usize,
    ) -> Result<(), String> {
        let stack_value = interpreter_stack
            .pop()
            .ok_or_else(|| "internal error: failed to get value from the stack".to_string())?;

        match stack_value {
            RibInterpreterStackValue::Val(ValueAndType {
                value: Value::List(items),
                typ: AnalysedType::List(typ),
            }) => {
                let value = items
                    .get(index)
                    .ok_or_else(|| {
                        format!(
                            "index {} is out of bound. list size: {}",
                            index,
                            items.len()
                        )
                    })?
                    .clone();

                interpreter_stack.push_val(ValueAndType::new(value, (*typ.inner).clone()));
                Ok(())
            }
            RibInterpreterStackValue::Val(ValueAndType {
                value: Value::Tuple(items),
                typ: AnalysedType::Tuple(typ),
            }) => {
                let value = items
                    .get(index)
                    .ok_or_else(|| format!("Index {} not found in the tuple", index))?
                    .clone();

                let item_type = typ
                    .items
                    .get(index)
                    .ok_or_else(|| format!("Index {} not found in the tuple type", index))?
                    .clone();

                interpreter_stack.push_val(ValueAndType::new(value, item_type));
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
                interpreter_stack.push_enum(enum_name, typed_enum.cases)?;
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
                    .ok_or_else(|| format!("unknown variant {} not found", variant_name))?;

                let variant_arg_typ = variant.typ.clone();

                let arg_value = match variant_arg_typ {
                    Some(_) => Some(interpreter_stack.try_pop_val()?),
                    None => None,
                };

                interpreter_stack.push_variant(
                    variant_name.clone(),
                    arg_value.map(|vnt| vnt.value),
                    variants.cases.clone(),
                )
            }

            _ => Err(format!(
                "internal error: expected a variant type for {}, but obtained {:?}",
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

                interpreter_stack.push_val(parsed_function_name.to_string().into_value_and_type());
            }

            FunctionReferenceType::RawResourceConstructor { resource } => {
                let parsed_function_name = ParsedFunctionName {
                    site,
                    function: ParsedFunctionReference::RawResourceConstructor { resource },
                };

                interpreter_stack.push_val(parsed_function_name.to_string().into_value_and_type());
            }
            FunctionReferenceType::RawResourceDrop { resource } => {
                let parsed_function_name = ParsedFunctionName {
                    site,
                    function: ParsedFunctionReference::RawResourceDrop { resource },
                };

                interpreter_stack.push_val(parsed_function_name.to_string().into_value_and_type());
            }
            FunctionReferenceType::RawResourceMethod { resource, method } => {
                let parsed_function_name = ParsedFunctionName {
                    site,
                    function: ParsedFunctionReference::RawResourceMethod { resource, method },
                };

                interpreter_stack.push_val(parsed_function_name.to_string().into_value_and_type());
            }
            FunctionReferenceType::RawResourceStaticMethod { resource, method } => {
                let parsed_function_name = ParsedFunctionName {
                    site,
                    function: ParsedFunctionReference::RawResourceStaticMethod { resource, method },
                };

                interpreter_stack.push_val(parsed_function_name.to_string().into_value_and_type());
            }
            FunctionReferenceType::IndexedResourceConstructor { resource, arg_size } => {
                let last_n_elements = interpreter_stack
                    .pop_n(arg_size)
                    .ok_or_else(|| "Failed to get values from the stack".to_string())?;

                let parameter_values = last_n_elements
                    .iter()
                    .map(|interpreter_result| {
                        interpreter_result.get_val().ok_or_else(|| {
                            "internal error: failed to construct resource".to_string()
                        })
                    })
                    .collect::<Result<Vec<ValueAndType>, String>>()?;

                let parsed_function_name = ParsedFunctionName {
                    site,
                    function: ParsedFunctionReference::IndexedResourceConstructor {
                        resource,
                        resource_params: parameter_values
                            .iter()
                            .map(print_value_and_type)
                            .collect::<Result<Vec<String>, String>>()?,
                    },
                };

                interpreter_stack.push_val(parsed_function_name.to_string().into_value_and_type());
            }
            FunctionReferenceType::IndexedResourceMethod {
                resource,
                arg_size,
                method,
            } => {
                let last_n_elements = interpreter_stack
                    .pop_n(arg_size)
                    .ok_or_else(|| "Failed to get values from the stack".to_string())?;

                let param_values = last_n_elements
                    .iter()
                    .map(|interpreter_result| {
                        interpreter_result.get_val().ok_or_else(|| {
                            "internal error: failed to call indexed resource method".to_string()
                        })
                    })
                    .collect::<Result<Vec<ValueAndType>, String>>()?;

                let parsed_function_name = ParsedFunctionName {
                    site,
                    function: ParsedFunctionReference::IndexedResourceMethod {
                        resource,
                        resource_params: param_values
                            .iter()
                            .map(print_value_and_type)
                            .collect::<Result<Vec<String>, String>>()?,
                        method,
                    },
                };

                interpreter_stack.push_val(parsed_function_name.to_string().into_value_and_type());
            }
            FunctionReferenceType::IndexedResourceStaticMethod {
                resource,
                arg_size,
                method,
            } => {
                let last_n_elements = interpreter_stack.pop_n(arg_size).ok_or_else(|| {
                    "internal error: Failed to get arguments for static resource method".to_string()
                })?;

                let param_values = last_n_elements
                    .iter()
                    .map(|interpreter_result| {
                        interpreter_result.get_val().ok_or_else(|| {
                            "internal error: Failed to call static resource method".to_string()
                        })
                    })
                    .collect::<Result<Vec<ValueAndType>, String>>()?;

                let parsed_function_name = ParsedFunctionName {
                    site,
                    function: ParsedFunctionReference::IndexedResourceStaticMethod {
                        resource,
                        resource_params: param_values
                            .iter()
                            .map(print_value_and_type)
                            .collect::<Result<Vec<String>, String>>()?,
                        method,
                    },
                };

                interpreter_stack.push_val(parsed_function_name.to_string().into_value_and_type());
            }
            FunctionReferenceType::IndexedResourceDrop { resource, arg_size } => {
                let last_n_elements = interpreter_stack.pop_n(arg_size).ok_or_else(|| {
                    "internal error: failed to get resource parameters for indexed resource drop"
                        .to_string()
                })?;

                let param_values = last_n_elements
                    .iter()
                    .map(|interpreter_result| {
                        interpreter_result.get_val().ok_or_else(|| {
                            "internal error: failed to call indexed resource drop".to_string()
                        })
                    })
                    .collect::<Result<Vec<ValueAndType>, String>>()?;

                let parsed_function_name = ParsedFunctionName {
                    site,
                    function: ParsedFunctionReference::IndexedResourceDrop {
                        resource,
                        resource_params: param_values
                            .iter()
                            .map(print_value_and_type)
                            .collect::<Result<Vec<String>, String>>()?,
                    },
                };

                interpreter_stack.push_val(parsed_function_name.to_string().into_value_and_type());
            }
        }

        Ok(())
    }

    pub(crate) async fn run_call_instruction(
        arg_size: usize,
        worker_type: WorkerNamePresence,
        interpreter_stack: &mut InterpreterStack,
        interpreter_env: &mut InterpreterEnv,
    ) -> Result<(), String> {
        let function_name = interpreter_stack
            .pop_str()
            .ok_or_else(|| "internal error: failed to get a function name".to_string())?;

        let worker_name = match worker_type {
            WorkerNamePresence::Present => {
                let worker_name = interpreter_stack
                    .pop_str()
                    .ok_or_else(|| "internal error: failed to get the worker name".to_string())?;

                Some(worker_name.clone())
            }
            WorkerNamePresence::Absent => None,
        };

        let last_n_elements = interpreter_stack.pop_n(arg_size).ok_or_else(|| {
            "internal error: failed to get arguments for the function call".to_string()
        })?;

        let parameter_values = last_n_elements
            .iter()
            .map(|interpreter_result| {
                interpreter_result.get_val().ok_or_else(|| {
                    format!("internal error: failed to call function {}", function_name)
                })
            })
            .collect::<Result<Vec<ValueAndType>, String>>()?;

        let result = interpreter_env
            .invoke_worker_function_async(worker_name, function_name, parameter_values)
            .await?;

        let interpreter_result = match result {
            ValueAndType {
                value: Value::Tuple(value),
                ..
            } if value.is_empty() => Ok(RibInterpreterStackValue::Unit),
            ValueAndType {
                value: Value::Tuple(value),
                typ: AnalysedType::Tuple(typ),
            } if value.len() == 1 => {
                let inner_value = value[0].clone();
                let inner_type = typ.items[0].clone();
                Ok(RibInterpreterStackValue::Val(ValueAndType::new(
                    inner_value,
                    inner_type,
                )))
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
            .ok_or_else(|| "Failed to get a value from the stack to unwrap".to_string())?;

        let unwrapped_value = value
            .unwrap()
            .ok_or_else(|| format!("Failed to unwrap the value {:?}", value))?;

        interpreter_stack.push_val(unwrapped_value);
        Ok(())
    }

    pub(crate) fn run_get_tag_instruction(
        interpreter_stack: &mut InterpreterStack,
    ) -> Result<(), String> {
        let value = interpreter_stack
            .pop_val()
            .ok_or_else(|| "Failed to get a tag value from the stack to unwrap".to_string())?;

        let tag = match value {
            ValueAndType {
                value: Value::Variant { case_idx, .. },
                typ: AnalysedType::Variant(typ),
            } => typ.cases[case_idx as usize].name.clone(),
            ValueAndType {
                value: Value::Option(option),
                ..
            } => match option {
                Some(_) => "some".to_string(),
                None => "none".to_string(),
            },
            ValueAndType {
                value: Value::Result(result_value),
                ..
            } => match result_value {
                Ok(_) => "ok".to_string(),
                Err(_) => "err".to_string(),
            },
            ValueAndType {
                value: Value::Enum(idx),
                typ: AnalysedType::Enum(typ),
            } => typ.cases[idx as usize].clone(),
            _ => "untagged".to_string(),
        };

        interpreter_stack.push_val(tag.into_value_and_type());
        Ok(())
    }

    pub(crate) fn run_create_some_instruction(
        interpreter_stack: &mut InterpreterStack,
        analysed_type: AnalysedType,
    ) -> Result<(), String> {
        let value = interpreter_stack.try_pop_val()?;

        match analysed_type {
            AnalysedType::Option(analysed_type) => {
                interpreter_stack.push_some(value.value, analysed_type.inner.deref());
                Ok(())
            }
            _ => Err(format!(
                "internal error: expected option type to create `some` value. But obtained {:?}",
                analysed_type
            )),
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
            _ => Err(format!(
                "internal error: expected option type to create `none` value. But obtained {:?}",
                analysed_type
            )),
        }
    }

    pub(crate) fn run_create_ok_instruction(
        interpreter_stack: &mut InterpreterStack,
        analysed_type: AnalysedType,
    ) -> Result<(), String> {
        let value = interpreter_stack.try_pop_val()?;

        match analysed_type {
            AnalysedType::Result(TypeResult { ok, err }) => {
                interpreter_stack.push_ok(value.value, ok.as_deref(), err.as_deref());
                Ok(())
            }
            _ => Err(format!(
                "internal error: expected result type to create `ok` value. But obtained {:?}",
                analysed_type
            )),
        }
    }

    pub(crate) fn run_create_err_instruction(
        interpreter_stack: &mut InterpreterStack,
        analysed_type: AnalysedType,
    ) -> Result<(), String> {
        let value = interpreter_stack.try_pop_val()?;

        match analysed_type {
            AnalysedType::Result(TypeResult { ok, err }) => {
                interpreter_stack.push_err(value.value, ok.as_deref(), err.as_deref());
                Ok(())
            }
            _ => Err(format!(
                "internal error: expected result type to create `err` value. But obtained {:?}",
                analysed_type
            )),
        }
    }

    pub(crate) fn run_concat_instruction(
        interpreter_stack: &mut InterpreterStack,
        arg_size: usize,
    ) -> Result<(), String> {
        let value_and_types = interpreter_stack.try_pop_n_val(arg_size)?;

        let mut result = String::new();

        for val in value_and_types {
            match &val.value {
                Value::String(s) => {
                    // Avoid extra quotes when concatenating strings
                    result.push_str(s);
                }
                Value::Char(char) => {
                    // Avoid extra single quotes when concatenating chars
                    result.push(*char);
                }
                _ => {
                    result.push_str(&val.to_string());
                }
            }
        }

        interpreter_stack.push_val(result.into_value_and_type());

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use test_r::test;

    use super::*;
    use crate::interpreter::rib_interpreter::tests::test_utils::{
        get_analysed_type_variant, get_metadata_with_enum_and_variant, get_value_and_type,
        strip_spaces,
    };
    use crate::{
        compiler, Expr, FunctionTypeRegistry, GlobalVariableTypeSpec, InferredType, InstructionId,
        Path, VariableId,
    };
    use golem_wasm_ast::analysis::analysed_type::{
        bool, case, f32, field, list, option, r#enum, record, result, s32, s8, str, tuple, u32,
        u64, u8, variant,
    };
    use golem_wasm_rpc::{parse_value_and_type, IntoValue, IntoValueAndType, Value, ValueAndType};

    #[test]
    async fn test_interpreter_for_literal() {
        let mut interpreter = Interpreter::default();

        let instructions = RibByteCode {
            instructions: vec![RibIR::PushLit(1i32.into_value_and_type())],
        };

        let result = interpreter.run(instructions).await.unwrap();
        assert_eq!(result.get_val().unwrap(), 1i32.into_value_and_type());
    }

    #[test]
    async fn test_interpreter_for_equal_to() {
        let mut interpreter = Interpreter::default();

        let instructions = RibByteCode {
            instructions: vec![
                RibIR::PushLit(1i32.into_value_and_type()),
                RibIR::PushLit(1u32.into_value_and_type()),
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
                RibIR::PushLit(1i32.into_value_and_type()),
                RibIR::PushLit(2u32.into_value_and_type()),
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
                RibIR::PushLit(2i32.into_value_and_type()),
                RibIR::PushLit(1u32.into_value_and_type()),
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
                RibIR::PushLit(2i32.into_value_and_type()),
                RibIR::PushLit(3u32.into_value_and_type()),
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
                RibIR::PushLit(2i32.into_value_and_type()), // rhs
                RibIR::PushLit(1i32.into_value_and_type()), // lhs
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
                RibIR::PushLit(1i32.into_value_and_type()),
                RibIR::AssignVar(VariableId::local_with_no_id("x")),
                RibIR::LoadVar(VariableId::local_with_no_id("x")),
            ],
        };

        let result = interpreter.run(instructions).await.unwrap();
        assert_eq!(result.get_val().unwrap(), 1i32.into_value_and_type());
    }

    #[test]
    async fn test_interpreter_for_jump() {
        let mut interpreter = Interpreter::default();

        let instructions = RibByteCode {
            instructions: vec![
                RibIR::Jump(InstructionId::init()),
                RibIR::PushLit(1i32.into_value_and_type()),
                RibIR::Label(InstructionId::init()),
            ],
        };

        let result = interpreter.run(instructions).await;
        assert!(result.is_ok());
    }

    #[test]
    async fn test_interpreter_for_jump_if_false() {
        let mut interpreter = Interpreter::default();

        let id = InstructionId::init().increment_mut();

        let instructions = RibByteCode {
            instructions: vec![
                RibIR::PushLit(false.into_value_and_type()),
                RibIR::JumpIfFalse(id.clone()),
                RibIR::PushLit(1i32.into_value_and_type()),
                RibIR::Label(id),
            ],
        };

        let result = interpreter.run(instructions).await;
        assert!(result.is_ok());
    }

    #[test]
    async fn test_interpreter_for_record() {
        let mut interpreter = Interpreter::default();

        let instructions = RibByteCode {
            instructions: vec![
                RibIR::PushLit(2i32.into_value_and_type()),
                RibIR::PushLit(1i32.into_value_and_type()),
                RibIR::CreateAndPushRecord(record(vec![field("x", s32()), field("y", s32())])),
                RibIR::UpdateRecord("x".to_string()),
                RibIR::UpdateRecord("y".to_string()),
            ],
        };

        let result = interpreter.run(instructions).await.unwrap();
        let expected = ValueAndType::new(
            Value::Record(vec![1i32.into_value(), 2i32.into_value()]),
            record(vec![field("x", s32()), field("y", s32())]),
        );

        assert_eq!(result.get_val().unwrap(), expected);
    }

    #[test]
    async fn test_interpreter_for_sequence() {
        let mut interpreter = Interpreter::default();

        let instructions = RibByteCode {
            instructions: vec![
                RibIR::PushLit(2i32.into_value_and_type()),
                RibIR::PushLit(1i32.into_value_and_type()),
                RibIR::PushList(list(s32()), 2),
            ],
        };

        let result = interpreter.run(instructions).await.unwrap();
        let expected = ValueAndType::new(
            Value::List(vec![1i32.into_value(), 2i32.into_value()]),
            list(s32()),
        );
        assert_eq!(result.get_val().unwrap(), expected);
    }

    #[test]
    async fn test_interpreter_for_select_field() {
        let mut interpreter = Interpreter::default();

        let instructions = RibByteCode {
            instructions: vec![
                RibIR::PushLit(1i32.into_value_and_type()),
                RibIR::PushLit(2i32.into_value_and_type()),
                RibIR::CreateAndPushRecord(record(vec![field("x", s32())])),
                RibIR::UpdateRecord("x".to_string()),
                RibIR::SelectField("x".to_string()),
            ],
        };

        let result = interpreter.run(instructions).await.unwrap();
        assert_eq!(result.get_val().unwrap(), 2i32.into_value_and_type());
    }

    #[test]
    async fn test_interpreter_for_select_index() {
        let mut interpreter = Interpreter::default();

        let instructions = RibByteCode {
            instructions: vec![
                RibIR::PushLit(1i32.into_value_and_type()),
                RibIR::PushLit(2i32.into_value_and_type()),
                RibIR::PushList(list(s32()), 2),
                RibIR::SelectIndex(0),
            ],
        };

        let result = interpreter.run(instructions).await.unwrap();
        assert_eq!(result.get_val().unwrap(), 2i32.into_value_and_type());
    }

    #[test]
    async fn test_interpreter_variable_scope_0() {
        let rib_expr = r#"
               let x: u64 = 1;
               let y = x + 2u64;
               y
            "#;

        let expr = Expr::from_text(rib_expr).unwrap();

        let compiled = compiler::compile(expr, &vec![]).unwrap();

        let mut interpreter = Interpreter::default();

        let result = interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(result.get_val().unwrap(), 3u64.into_value_and_type());
    }

    #[test]
    async fn test_interpreter_variable_scope_1() {
        let rib_expr = r#"
               let x: u64 = 1;
               let z = {foo : x};
               let x = x + 2u64;
               { bar: x, baz: z }
            "#;

        let expr = Expr::from_text(rib_expr).unwrap();

        let compiled = compiler::compile(expr, &vec![]).unwrap();

        let mut interpreter = Interpreter::default();

        let result = interpreter.run(compiled.byte_code).await.unwrap();

        let analysed_type = record(vec![
            field("bar", u64()),
            field("baz", record(vec![field("foo", u64())])),
        ]);

        let expected = get_value_and_type(&analysed_type, r#"{ bar: 3, baz: { foo: 1 } }"#);

        assert_eq!(result.get_val().unwrap(), expected);
    }

    #[test]
    async fn test_interpreter_variable_scope_2() {
        let rib_expr = r#"
               let x: u64 = 1;
               let x = x;

               let result1 = match some(x + 1:u64) {
                  some(x) => x,
                  none => x
               };

               let z: option<u64> = none;

               let result2 = match z {
                  some(x) => x,
                  none => x
               };

               { result1: result1, result2: result2 }
            "#;

        let expr = Expr::from_text(rib_expr).unwrap();

        let compiled = compiler::compile(expr, &vec![]).unwrap();

        let mut interpreter = Interpreter::default();

        let result = interpreter.run(compiled.byte_code).await.unwrap();

        let analysed_type = record(vec![field("result1", u64()), field("result2", u64())]);

        let expected = get_value_and_type(&analysed_type, r#"{ result1: 2, result2: 1 }"#);

        assert_eq!(result.get_val().unwrap(), expected);
    }

    #[test]
    async fn test_interpreter_variable_scope_3() {
        let rib_expr = r#"
               let x: u64 = 1;
               let x = x;

               let result1 = match some(x + 1:u64) {
                  some(x) => match some(x + 1:u64) {
                     some(x) => x,
                     none => x
                  },
                  none => x
               };

               let z: option<u64> = none;

               let result2 = match z {
                  some(x) => x,
                  none => match some(x + 1:u64) {
                     some(x) => x,
                     none => x
                  }
               };

               { result1: result1, result2: result2 }
            "#;

        let expr = Expr::from_text(rib_expr).unwrap();

        let compiled = compiler::compile(expr, &vec![]).unwrap();

        let mut interpreter = Interpreter::default();

        let result = interpreter.run(compiled.byte_code).await.unwrap();

        let analysed_type = record(vec![field("result1", u64()), field("result2", u64())]);

        let expected = get_value_and_type(&analysed_type, r#"{ result1: 3, result2: 2 }"#);

        assert_eq!(result.get_val().unwrap(), expected);
    }

    #[test]
    async fn test_interpreter_global_variable_with_type_spec() {
        // request.path.user-id and request.headers.* should be inferred as string,
        // since we configure the compiler with a type-spec (given below)
        let rib_expr = r#"
               let res1 = request.path.user-id;
               let res2 = request.headers.name;
               let res3 = request.headers.age;
               "${res1}-${res2}-${res3}"
            "#;

        let type_spec = vec![
            GlobalVariableTypeSpec {
                variable_id: VariableId::global("request".to_string()),
                path: Path::from_elems(vec!["path"]),
                inferred_type: InferredType::Str,
            },
            GlobalVariableTypeSpec {
                variable_id: VariableId::global("request".to_string()),
                path: Path::from_elems(vec!["headers"]),
                inferred_type: InferredType::Str,
            },
        ];

        let mut rib_input = HashMap::new();

        // Rib compiler identifies the input requirements to be a string (due to type-spec passed)
        // and therefore, we pass input value (value_and_type) to the interpreter with headers and path values as string
        let analysed_type_of_input = &record(vec![
            field("path", record(vec![field("user-id", str())])),
            field(
                "headers",
                record(vec![field("name", str()), field("age", str())]),
            ),
        ]);

        let value_and_type = get_value_and_type(
            analysed_type_of_input,
            r#"{path : { user-id: "1" }, headers: { name: "foo", age: "20" }}"#,
        );

        rib_input.insert("request".to_string(), value_and_type);

        let mut interpreter = test_utils::interpreter_static_response(
            &ValueAndType::new(Value::S8(1), s8()),
            Some(RibInput::new(rib_input)),
        );

        let expr = Expr::from_text(rib_expr).unwrap();

        let compiled =
            compiler::compile_with_restricted_global_variables(expr, &vec![], None, &type_spec)
                .unwrap();

        let result = interpreter
            .run(compiled.byte_code)
            .await
            .unwrap()
            .get_val()
            .unwrap()
            .value;

        assert_eq!(result, Value::String("1-foo-20".to_string()))
    }

    #[test]
    async fn test_interpreter_global_variable_override_type_spec() {
        let rib_expr = r#"
             let res1: u32 = request.path.user-id;
             let res2 = request.headers.name;
             let res3: u32 = request.headers.age;
             let res4 = res1 + res3;
             "${res4}-${res2}"
            "#;

        // We always specify the type of request.path.* and request.headers.* to be a string using type-spec
        // however the rib script (above) explicitly specify the type of request.path.user-id
        // and request.header.age to be u32. In this case, the Rib compiler infer them as u32 and interpreter works with u32.
        let type_spec = vec![
            GlobalVariableTypeSpec {
                variable_id: VariableId::global("request".to_string()),
                path: Path::from_elems(vec!["path"]),
                inferred_type: InferredType::Str,
            },
            GlobalVariableTypeSpec {
                variable_id: VariableId::global("request".to_string()),
                path: Path::from_elems(vec!["headers"]),
                inferred_type: InferredType::Str,
            },
        ];

        let mut rib_input = HashMap::new();

        // We pass the input value to rib-interpreter with request.path.user-id
        // and request.headers.age as u32, since the compiler inferred these input type requirements to be u32.
        let analysed_type_of_input = &record(vec![
            field("path", record(vec![field("user-id", u32())])),
            field(
                "headers",
                record(vec![field("name", str()), field("age", u32())]),
            ),
        ]);

        let value_and_type = get_value_and_type(
            analysed_type_of_input,
            r#"{path : { user-id: 1 }, headers: { name: "foo", age: 20 }}"#,
        );

        rib_input.insert("request".to_string(), value_and_type);

        let mut interpreter = test_utils::interpreter_static_response(
            &ValueAndType::new(Value::S8(1), s8()),
            Some(RibInput::new(rib_input)),
        );

        let expr = Expr::from_text(rib_expr).unwrap();

        let compiled =
            compiler::compile_with_restricted_global_variables(expr, &vec![], None, &type_spec)
                .unwrap();

        let result = interpreter
            .run(compiled.byte_code)
            .await
            .unwrap()
            .get_val()
            .unwrap()
            .value;

        assert_eq!(result, Value::String("21-foo".to_string()))
    }

    #[test]
    async fn test_interpreter_concatenation() {
        let mut interpreter = test_utils::interpreter_dynamic_response(None);

        let rib_expr = r#"
            let x = "foo";
            let y = "bar";
            let z = {foo: "baz"};
            let n: u32 = 42;
            let result = "${x}-${y}-${z}-${n}";
            result
        "#;

        let expr = Expr::from_text(rib_expr).unwrap();
        let compiled = compiler::compile(expr, &vec![]).unwrap();
        let result = interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(
            result.get_val().unwrap().value,
            Value::String("foo-bar-{foo: \"baz\"}-42".to_string())
        );
    }

    #[test]
    async fn test_interpreter_with_variant_and_enum() {
        let mut interpreter = test_utils::interpreter_dynamic_response(None);

        // This has intentionally got conflicting variable names
        // variable `x` is same as the enum name `x`
        // similarly, variably `validate` is same as the variant name validate
        let expr = r#"
          let x = x;
          let y = x;
          let result1 = add-enum(x, y);
          let validate = validate;
          let validate2 = validate;
          let result2 = add-variant(validate, validate2);
          {res1: result1, res2: result2}
        "#;

        let expr = Expr::from_text(expr).unwrap();
        let compiled = compiler::compile(expr, &get_metadata_with_enum_and_variant()).unwrap();
        let result = interpreter.run(compiled.byte_code).await.unwrap();
        let expected_enum_type = r#enum(&["x", "y", "z"]);
        let expected_variant_type = get_analysed_type_variant();

        let expected_record_type = record(vec![
            field("res1", expected_enum_type),
            field("res2", expected_variant_type),
        ]);

        let expected_record_value = Value::Record(vec![
            Value::Enum(0),
            Value::Variant {
                case_idx: 2,
                case_value: None,
            },
        ]);

        assert_eq!(
            result,
            RibResult::Val(ValueAndType::new(
                expected_record_value,
                expected_record_type
            ))
        );
    }

    #[test]
    async fn test_interpreter_with_conflicting_variable_names() {
        let mut interpreter = test_utils::interpreter_dynamic_response(None);

        // This has intentionally conflicting variable names
        // variable `x` is same as the enum name `x`
        // similarly, variably `validate` is same as the variant name `validate`
        // and `process-user` is same as the variant name `process-user`
        let expr = r#"
          let x = 1;
          let y = 2;
          let result1 = add-u32(x, y);
          let process-user = 3;
          let validate = 4;
          let result2 = add-u64(process-user, validate);
          {res1: result1, res2: result2}
        "#;

        let expr = Expr::from_text(expr).unwrap();
        let compiled = compiler::compile(expr, &get_metadata_with_enum_and_variant()).unwrap();
        let result = interpreter.run(compiled.byte_code).await.unwrap();
        let expected_value = Value::Record(vec![3u32.into_value(), 7u64.into_value()]);

        let expected_type = record(vec![field("res1", u32()), field("res2", u64())]);
        assert_eq!(
            result,
            RibResult::Val(ValueAndType::new(expected_value, expected_type))
        );
    }

    #[test]
    async fn test_interpreter_list_reduce() {
        let mut interpreter = Interpreter::default();

        let rib_expr = r#"
          let x: list<u8> = [1, 2];

          reduce z, a in x from 0u8 {
            yield z + a;
          }

          "#;

        let expr = Expr::from_text(rib_expr).unwrap();

        let compiled = compiler::compile(expr, &vec![]).unwrap();

        let result = interpreter
            .run(compiled.byte_code)
            .await
            .unwrap()
            .get_val()
            .unwrap();

        assert_eq!(result, 3u8.into_value_and_type());
    }

    #[test]
    async fn test_interpreter_list_reduce_from_record() {
        let mut interpreter = Interpreter::default();

        let rib_expr = r#"
           let x = [{name: "foo", age: 1u64}, {name: "bar", age: 2u64}];

           let names = for i in x {
             yield i.name;
           };

          reduce z, a in names from "" {
            let result = if z == "" then a else "${z}, ${a}";

            yield result;
          }

          "#;

        let expr = Expr::from_text(rib_expr).unwrap();

        let compiled = compiler::compile(expr, &vec![]).unwrap();

        let result = interpreter
            .run(compiled.byte_code)
            .await
            .unwrap()
            .get_val()
            .unwrap();

        assert_eq!(result, "foo, bar".into_value_and_type());
    }

    #[test]
    async fn test_interpreter_list_reduce_text() {
        let mut interpreter = Interpreter::default();

        let rib_expr = r#"
           let x = ["foo", "bar"];

          reduce z, a in x from "" {
            let result = if z == "" then a else "${z}, ${a}";

            yield result;
          }

          "#;

        let expr = Expr::from_text(rib_expr).unwrap();

        let compiled = compiler::compile(expr, &vec![]).unwrap();

        let result = interpreter
            .run(compiled.byte_code)
            .await
            .unwrap()
            .get_val()
            .unwrap();

        assert_eq!(result, "foo, bar".into_value_and_type());
    }

    #[test]
    async fn test_interpreter_list_reduce_empty() {
        let mut interpreter = Interpreter::default();

        let rib_expr = r#"
          let x: list<u8> = [];

          reduce z, a in x from 0u8 {
            yield z + a;
          }

          "#;

        let expr = Expr::from_text(rib_expr).unwrap();

        let compiled = compiler::compile(expr, &vec![]).unwrap();

        let result = interpreter
            .run(compiled.byte_code)
            .await
            .unwrap()
            .get_val()
            .unwrap();

        assert_eq!(result, 0u8.into_value_and_type());
    }

    #[test]
    async fn test_interpreter_with_numbers_1() {
        let component_metadata =
            test_utils::get_component_metadata("foo", vec![u32()], Some(u64()));

        let mut interpreter =
            test_utils::interpreter_static_response(&ValueAndType::new(Value::U64(2), u64()), None);

        // 1 is automatically inferred to be u32
        let rib = r#"
          let worker = instance("my-worker");
          worker.foo(1)
        "#;

        let expr = Expr::from_text(rib).unwrap();
        let compiled = compiler::compile(expr, &component_metadata).unwrap();
        let result = interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(
            result.get_val().unwrap(),
            ValueAndType::new(Value::U64(2), u64())
        );
    }

    #[test]
    async fn test_interpreter_with_numbers_2() {
        let component_metadata =
            test_utils::get_component_metadata("foo", vec![u32()], Some(u64()));

        let mut interpreter =
            test_utils::interpreter_static_response(&ValueAndType::new(Value::U64(2), u64()), None);

        // 1 and 2 are automatically inferred to be u32
        // since the type of z is inferred to be u32 as that being passed to a function
        // that expects u32
        let rib = r#"
          let worker = instance("my-worker");
          let z = 1 + 2;
          worker.foo(z)
        "#;

        let expr = Expr::from_text(rib).unwrap();
        let compiled = compiler::compile(expr, &component_metadata).unwrap();
        let result = interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(
            result.get_val().unwrap(),
            ValueAndType::new(Value::U64(2), u64())
        );
    }

    #[test]
    async fn test_interpreter_with_numbers_3() {
        let component_metadata =
            test_utils::get_component_metadata("foo", vec![u32()], Some(u64()));

        // This will cause a type inference error
        // because the operands of the + operator are not of the same type
        let rib = r#"
          let worker = instance("my-worker");
          let z = 1: u8 + 2;
          worker.foo(z)
        "#;

        let expr = Expr::from_text(rib).unwrap();
        let compile_result = compiler::compile(expr, &component_metadata);
        assert!(compile_result.is_err());
    }

    #[test]
    async fn test_interpreter_with_numbers_4() {
        let component_metadata =
            test_utils::get_component_metadata("foo", vec![u32()], Some(u64()));

        // This will cause a type inference error
        // because the operands of the + operator are supposed to be u32
        // since z is u32
        let rib = r#"
          let worker = instance("my-worker");
          let z = 1: u8 + 2: u8;
          worker.foo(z)
        "#;

        let expr = Expr::from_text(rib).unwrap();
        let compile_result = compiler::compile(expr, &component_metadata);
        assert!(compile_result.is_err());
    }

    #[test]
    async fn test_interpreter_list_comprehension() {
        let mut interpreter = Interpreter::default();

        let rib_expr = r#"
          let x = ["foo", "bar"];

          for i in x {
            yield i;
          }

          "#;

        let expr = Expr::from_text(rib_expr).unwrap();

        let compiled = compiler::compile(expr, &vec![]).unwrap();

        let result = interpreter
            .run(compiled.byte_code)
            .await
            .unwrap()
            .get_val()
            .unwrap();

        let expected = r#"["foo", "bar"]"#;
        let expected_value = golem_wasm_rpc::parse_value_and_type(&list(str()), expected).unwrap();

        assert_eq!(result, expected_value);
    }

    #[test]
    async fn test_interpreter_list_comprehension_empty() {
        let mut interpreter = Interpreter::default();

        let rib_expr = r#"
          let x: list<string> = [];

          for i in x {
            yield i;
          }

          "#;

        let expr = Expr::from_text(rib_expr).unwrap();

        let compiled = compiler::compile(expr, &vec![]).unwrap();

        let result = interpreter
            .run(compiled.byte_code)
            .await
            .unwrap()
            .get_val()
            .unwrap();

        let expected = r#"[]"#;
        let expected_value_and_type =
            golem_wasm_rpc::parse_value_and_type(&list(str()), expected).unwrap();

        assert_eq!(result, expected_value_and_type);
    }

    #[test]
    async fn test_interpreter_pattern_match_on_option_nested() {
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
        expr.infer_types(&FunctionTypeRegistry::empty(), &vec![])
            .unwrap();
        let compiled = compiler::compile(expr, &vec![]).unwrap();
        let result = interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(result.get_val().unwrap(), 0u64.into_value_and_type());
    }

    #[test]
    async fn test_interpreter_pattern_match_on_tuple() {
        let mut interpreter = Interpreter::default();

        let expr = r#"
           let x: tuple<u64, string, string> = (1, "foo", "bar");

           match x {
              (x, y, z) => "${x} ${y} ${z}"
           }
        "#;

        let mut expr = Expr::from_text(expr).unwrap();
        expr.infer_types(&FunctionTypeRegistry::empty(), &vec![])
            .unwrap();
        let compiled = compiler::compile(expr, &vec![]).unwrap();
        let result = interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(result.get_val().unwrap(), "1 foo bar".into_value_and_type());
    }

    #[test]
    async fn test_interpreter_pattern_match_on_tuple_with_option_some() {
        let mut interpreter = Interpreter::default();

        let expr = r#"
           let x: tuple<u64, option<string>, string> = (1, some("foo"), "bar");

           match x {
              (x, none, z) => "${x} ${z}",
              (x, some(y), z) => "${x} ${y} ${z}"
           }
        "#;

        let mut expr = Expr::from_text(expr).unwrap();
        expr.infer_types(&FunctionTypeRegistry::empty(), &vec![])
            .unwrap();

        let compiled = compiler::compile(expr, &vec![]).unwrap();
        let result = interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(result.get_val().unwrap(), "1 foo bar".into_value_and_type());
    }

    #[test]
    async fn test_interpreter_pattern_match_on_tuple_with_option_none() {
        let mut interpreter = Interpreter::default();

        let expr = r#"
           let x: tuple<u64, option<string>, string> = (1, none, "bar");

           match x {
              (x, none, z) => "${x} ${z}",
              (x, some(y), z) => "${x} ${y} ${z}"
           }
        "#;

        let expr = Expr::from_text(expr).unwrap();
        let compiled = compiler::compile(expr, &vec![]).unwrap();
        let result = interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(result.get_val().unwrap(), "1 bar".into_value_and_type());
    }

    #[test]
    async fn test_interpreter_pattern_match_dynamic_branch_1() {
        let mut interpreter = Interpreter::default();

        let expr = r#"
           let x: u64 = 1;

           match x {
                1 => ok(1: u64),
                2 => err("none")
           }
        "#;

        let expr = Expr::from_text(expr).unwrap();
        let compiled = compiler::compile(expr, &vec![]).unwrap();
        let rib_result = interpreter.run(compiled.byte_code).await.unwrap();

        let expected = ValueAndType::new(
            Value::Result(Ok(Some(Box::new(Value::U64(1))))),
            result(u64(), str()),
        );

        assert_eq!(rib_result.get_val().unwrap(), expected);
    }

    #[test]
    async fn test_interpreter_pattern_match_dynamic_branch_2() {
        let mut interpreter = Interpreter::default();

        let expr = r#"
           let x = some({foo: 1:u64});

           match x {
               some(x) => ok(x.foo),
               none => err("none")
           }
        "#;

        let expr = Expr::from_text(expr).unwrap();
        let compiled = compiler::compile(expr, &vec![]).unwrap();
        let rib_result = interpreter.run(compiled.byte_code).await.unwrap();

        let expected = ValueAndType::new(
            Value::Result(Ok(Some(Box::new(Value::U64(1))))),
            result(u64(), str()),
        );

        assert_eq!(rib_result.get_val().unwrap(), expected);
    }

    #[test]
    async fn test_interpreter_pattern_match_on_tuple_with_all_types() {
        let mut interpreter = Interpreter::default();

        let tuple = test_utils::get_analysed_type_tuple();

        let analysed_exports = test_utils::get_component_metadata("foo", vec![tuple], Some(str()));

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
        let compiled = compiler::compile(expr, &analysed_exports).unwrap();
        let result = interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(
            result.get_val().unwrap(),
            "foo 100 1 bar jak validate prod dev test".into_value_and_type()
        );
    }

    #[test]
    async fn test_interpreter_pattern_match_on_tuple_with_wild_pattern() {
        let mut interpreter = Interpreter::default();

        let tuple = test_utils::get_analysed_type_tuple();

        let analysed_exports =
            test_utils::get_component_metadata("my-worker-function", vec![tuple], Some(str()));

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
        let compiled = compiler::compile(expr, &analysed_exports).unwrap();
        let result = interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(
            result.get_val().unwrap(),
            "dev 1 bar jak baz".into_value_and_type()
        );
    }

    #[test]
    async fn test_interpreter_record_output_in_pattern_match() {
        let input_analysed_type = test_utils::get_analysed_type_record();
        let output_analysed_type = test_utils::get_analysed_type_result();

        let result_value = get_value_and_type(&output_analysed_type, r#"ok(1)"#);

        let mut interpreter = test_utils::interpreter_static_response(&result_value, None);

        let analysed_exports = test_utils::get_component_metadata(
            "my-worker-function",
            vec![input_analysed_type],
            Some(output_analysed_type),
        );

        let expr = r#"

           let input = { request : { path : { user : "jak" } }, y : "baz" };
           let result = my-worker-function(input);
           match result {
             ok(result) => { body: result, status: 200 },
             err(result) => { status: 400, body: 400 }
           }
        "#;

        let expr = Expr::from_text(expr).unwrap();
        let compiled = compiler::compile(expr, &analysed_exports).unwrap();
        let result = interpreter.run(compiled.byte_code).await.unwrap();

        let expected = test_utils::get_value_and_type(
            &record(vec![field("body", u64()), field("status", u64())]),
            r#"{body: 1, status: 200}"#,
        );

        assert_eq!(result.get_val().unwrap(), expected);
    }

    #[test]
    async fn test_interpreter_tuple_output_in_pattern_match() {
        let input_analysed_type = test_utils::get_analysed_type_record();
        let output_analysed_type = test_utils::get_analysed_type_result();

        let result_value = get_value_and_type(&output_analysed_type, r#"err("failed")"#);

        let mut interpreter = test_utils::interpreter_static_response(&result_value, None);

        let analysed_exports = test_utils::get_component_metadata(
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
        let compiled = compiler::compile(expr, &analysed_exports).unwrap();
        let result = interpreter.run(compiled.byte_code).await.unwrap();

        let expected = get_value_and_type(&tuple(vec![str(), str()]), r#"("failed", "bar")"#);

        assert_eq!(result.get_val().unwrap(), expected);
    }

    #[test]
    async fn test_interpreter_with_indexed_resource_drop() {
        let expr = r#"
           let user_id = "user";
           golem:it/api.{cart(user_id).drop}();
           "success"
        "#;
        let expr = Expr::from_text(expr).unwrap();
        let component_metadata = test_utils::get_metadata_with_resource_with_params();

        let compiled = compiler::compile(expr, &component_metadata).unwrap();

        let mut rib_interpreter = Interpreter::default();
        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(result.get_val().unwrap(), "success".into_value_and_type());
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

        let result_value = test_utils::get_value_and_type(
            &result_type,
            r#"
          success({order-id: "foo"})
        "#,
        );

        let component_metadata = test_utils::get_metadata_with_resource_with_params();
        let compiled = compiler::compile(expr, &component_metadata).unwrap();

        let mut rib_executor = test_utils::interpreter_static_response(&result_value, None);
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

        let result_value = test_utils::get_value_and_type(
            &result_type,
            r#"
            [{product-id: "foo", name: "bar", price: 100.0, quantity: 1}, {product-id: "bar", name: "baz", price: 200.0, quantity: 2}]
        "#,
        );

        let component_metadata = test_utils::get_metadata_with_resource_with_params();
        let compiled = compiler::compile(expr, &component_metadata).unwrap();

        let mut rib_executor = test_utils::interpreter_static_response(&result_value, None);
        let result = rib_executor.run(compiled.byte_code).await.unwrap();

        assert_eq!(result.get_val().unwrap(), "foo".into_value_and_type());
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

        let component_metadata = test_utils::get_metadata_with_resource_with_params();

        let compiled = compiler::compile(expr, &component_metadata).unwrap();

        let mut rib_executor = Interpreter::default();

        let result = rib_executor.run(compiled.byte_code).await.unwrap();

        assert_eq!(
            result.get_val().unwrap(),
            "successfully updated".into_value_and_type()
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

        let component_metadata = test_utils::get_metadata_with_resource_with_params();

        let compiled = compiler::compile(expr, &component_metadata).unwrap();

        let mut rib_executor = Interpreter::default();

        let result = rib_executor.run(compiled.byte_code).await.unwrap();

        assert_eq!(
            result.get_val().unwrap(),
            "successfully added".into_value_and_type()
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

        let component_metadata = test_utils::get_metadata_with_resource_without_params();

        let compiled = compiler::compile(expr, &component_metadata).unwrap();

        let mut rib_executor = Interpreter::default();

        let result = rib_executor.run(compiled.byte_code).await.unwrap();

        assert_eq!(
            result.get_val().unwrap(),
            "successfully added".into_value_and_type()
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

        let result_value = test_utils::get_value_and_type(
            &result_type,
            r#"
            [{product-id: "foo", name: "bar", price: 100.0, quantity: 1}, {product-id: "bar", name: "baz", price: 200.0, quantity: 2}]
        "#,
        );

        let component_metadata = test_utils::get_metadata_with_resource_without_params();
        let compiled = compiler::compile(expr, &component_metadata).unwrap();

        let mut rib_executor = test_utils::interpreter_static_response(&result_value, None);
        let result = rib_executor.run(compiled.byte_code).await.unwrap();

        assert_eq!(result.get_val().unwrap(), "foo".into_value_and_type());
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

        let component_metadata = test_utils::get_metadata_with_resource_without_params();

        let compiled = compiler::compile(expr, &component_metadata).unwrap();

        let mut rib_executor = Interpreter::default();

        let result = rib_executor.run(compiled.byte_code).await.unwrap();

        assert_eq!(
            result.get_val().unwrap(),
            "successfully updated".into_value_and_type()
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

        let result_value = test_utils::get_value_and_type(
            &result_type,
            r#"
          success({order-id: "foo"})
        "#,
        );

        let component_metadata = test_utils::get_metadata_with_resource_without_params();
        let compiled = compiler::compile(expr, &component_metadata).unwrap();

        let mut rib_executor = test_utils::interpreter_static_response(&result_value, None);
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
        let component_metadata = test_utils::get_metadata_with_resource_without_params();

        let compiled = compiler::compile(expr, &component_metadata).unwrap();

        let mut rib_interpreter = Interpreter::default();
        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(result.get_val().unwrap(), "success".into_value_and_type());
    }

    #[test]
    async fn test_interpreter_for_select_index_expr_1() {
        // infinite computation will respond with an error - than a stack overflow
        // Note that, `list[1..]` is allowed while `for i in 1.. { yield i; }` is not
        let expr = r#"
              let list: list<u8> = [1, 2, 3, 4, 5];
              let index: u8 = 4;
              list[index]
              "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiled = compiler::compile(expr, &vec![]).unwrap();

        let mut interpreter = Interpreter::default();
        let result = interpreter.run(compiled.byte_code).await.unwrap();

        let expected = ValueAndType::new(Value::U8(5), u8());

        assert_eq!(result.get_val().unwrap(), expected);
    }

    #[test]
    async fn test_interpreter_for_select_index_expr_out_of_bound() {
        let expr = r#"
              let list: list<u8> = [1, 2, 3, 4, 5];
              let index: u8 = 10;
              list[index]
              "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiled = compiler::compile(expr, &vec![]).unwrap();

        let mut interpreter = Interpreter::default();
        let result = interpreter.run(compiled.byte_code).await.unwrap_err();

        assert_eq!(
            result,
            "index 10 is out of bound in the list of length 5".to_string()
        );
    }

    #[test]
    async fn test_interpreter_for_select_index_expr_2() {
        let expr = r#"
              let list: list<u8> = [1, 2, 3, 4, 5];
              let indices: list<u8> = [0, 1, 2, 3];

              for i in indices {
                 yield list[i];
              }
              "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiled = compiler::compile(expr, &vec![]).unwrap();

        let mut interpreter = Interpreter::default();
        let result = interpreter.run(compiled.byte_code).await.unwrap();

        let expected = ValueAndType::new(
            Value::List(vec![Value::U8(1), Value::U8(2), Value::U8(3), Value::U8(4)]),
            list(u8()),
        );

        assert_eq!(result.get_val().unwrap(), expected);
    }

    #[test]
    async fn test_interpreter_for_select_index_expr_3() {
        let expr = r#"
              let list: list<u8> = [2, 5, 4];
              let indices: list<u8> = [0, 1];

               reduce z, index in indices from 0u8 {
                  yield list[index] + z;
                }
              "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiled = compiler::compile(expr, &vec![]).unwrap();

        let mut interpreter = Interpreter::default();
        let result = interpreter.run(compiled.byte_code).await.unwrap();

        let expected = ValueAndType::new(Value::U8(7), u8());

        assert_eq!(result.get_val().unwrap(), expected);
    }

    #[test]
    async fn test_interpreter_for_select_index_expr_4() {
        let expr = r#"
              let list: list<u8> = [2, 5, 4];
              let x: u8 = 0;
              let y: u8 = 2;
              list[x..=y]
              "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiled = compiler::compile(expr, &vec![]).unwrap();

        let mut interpreter = Interpreter::default();
        let result = interpreter.run(compiled.byte_code).await.unwrap();

        let expected = ValueAndType::new(
            Value::List(vec![Value::U8(2), Value::U8(5), Value::U8(4)]),
            list(u8()),
        );

        assert_eq!(result.get_val().unwrap(), expected);
    }

    #[test]
    async fn test_interpreter_for_select_index_expr_5() {
        let expr = r#"
              let list: list<u8> = [2, 5, 4];
              let x: u8 = 0;
              let y: u8 = 2;
              let x1: u8 = 1;
              let result = list[x..=y];
              for i in result[x1..=y] {
                yield i;
              }
              "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiled = compiler::compile(expr, &vec![]).unwrap();

        let mut interpreter = Interpreter::default();
        let result = interpreter.run(compiled.byte_code).await.unwrap();

        let expected = ValueAndType::new(Value::List(vec![Value::U8(5), Value::U8(4)]), list(u8()));

        assert_eq!(result.get_val().unwrap(), expected);
    }

    #[test]
    async fn test_interpreter_for_select_index_expr_6() {
        let expr = r#"
              let list: list<u8> = [2, 5, 4, 6];
              let x: u8 = 0;
              let y: u8 = 2;
              let result = list[x..y];
              for i in result[x..y] {
                yield i;
              }
              "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiled = compiler::compile(expr, &vec![]).unwrap();

        let mut interpreter = Interpreter::default();
        let result = interpreter.run(compiled.byte_code).await.unwrap();

        let expected = ValueAndType::new(Value::List(vec![Value::U8(2), Value::U8(5)]), list(u8()));

        assert_eq!(result.get_val().unwrap(), expected);
    }

    #[test]
    async fn test_interpreter_for_select_index_expr_7() {
        let expr = r#"
              let list: list<u8> = [2, 5, 4, 6];
              let x: u8 = 0;
              let result = list[x..];
              for i in result[x..] {
                yield i;
              }
              "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiled = compiler::compile(expr, &vec![]).unwrap();

        let mut interpreter = Interpreter::default();
        let result = interpreter.run(compiled.byte_code).await.unwrap();

        let expected = ValueAndType::new(Value::List(vec![Value::U8(2), Value::U8(5)]), list(u8()));

        assert_eq!(result.get_val().unwrap(), expected);
    }

    #[test]
    async fn test_interpreter_for_select_index_expr_8() {
        let expr = r#"
              let list: list<u8> = [2, 5, 4, 6];
              let result = list[0..2];
              for i in result[0..2] {
                yield i;
              }
              "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiled = compiler::compile(expr, &vec![]).unwrap();

        let mut interpreter = Interpreter::default();
        let result = interpreter.run(compiled.byte_code).await.unwrap();

        let expected = ValueAndType::new(Value::List(vec![Value::U8(2), Value::U8(5)]), list(u8()));

        assert_eq!(result.get_val().unwrap(), expected);
    }

    // Simulating the behaviour in languages like rust
    // Emitting the description of the range than the evaluated range
    // Description given out as ValueAndType::Record
    #[test]
    async fn test_interpreter_range_returns_1() {
        let expr = r#"
              let x = 1..;
              x
              "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiled = compiler::compile(expr, &vec![]).unwrap();

        let mut interpreter = Interpreter::default();
        let result = interpreter.run(compiled.byte_code).await.unwrap();

        let expected = ValueAndType::new(
            Value::Record(vec![
                Value::U64(1),
                Value::Bool(false), // non inclusive
            ]),
            record(vec![
                field("from", option(u64())),
                field("inclusive", bool()),
            ]),
        );

        assert_eq!(result.get_val().unwrap(), expected);
    }

    #[test]
    async fn test_interpreter_range_returns_2() {
        let expr = r#"
              let x = 1..2;
              x
              "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiled = compiler::compile(expr, &vec![]).unwrap();

        let mut interpreter = Interpreter::default();
        let result = interpreter.run(compiled.byte_code).await.unwrap();

        let expected = ValueAndType::new(
            Value::Record(vec![
                Value::U64(1),
                Value::U64(2),
                Value::Bool(false), // non inclusive
            ]),
            record(vec![
                field("from", option(u64())),
                field("to", option(u64())),
                field("inclusive", bool()),
            ]),
        );

        assert_eq!(result.get_val().unwrap(), expected);
    }

    #[test]
    async fn test_interpreter_range_returns_3() {
        let expr = r#"
              let x = 1..=10;
              x
              "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiled = compiler::compile(expr, &vec![]).unwrap();

        let mut interpreter = Interpreter::default();
        let result = interpreter.run(compiled.byte_code).await.unwrap();

        let expected = ValueAndType::new(
            Value::Record(vec![
                Value::U64(1),
                Value::U64(10),
                Value::Bool(true), // inclusive
            ]),
            record(vec![
                field("from", option(u64())),
                field("to", option(u64())),
                field("inclusive", bool()),
            ]),
        );

        assert_eq!(result.get_val().unwrap(), expected);
    }

    #[test]
    async fn test_interpreter_range_returns_4() {
        let expr = r#"
              let x = 1:u64;
              let y = x;
              let range = x..=y;
              let range2 = x..;
              let range3 = x..y;
              range;
              range2;
              range3
              "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiled = compiler::compile(expr, &vec![]).unwrap();

        let mut interpreter = Interpreter::default();
        let result = interpreter.run(compiled.byte_code).await.unwrap();

        let expected = ValueAndType::new(
            Value::Record(vec![Value::U64(1), Value::U64(1), Value::Bool(false)]),
            record(vec![
                field("from", option(u64())),
                field("to", option(u64())),
                field("inclusive", bool()),
            ]),
        );

        assert_eq!(result.get_val().unwrap(), expected);
    }

    #[test]
    async fn test_interpreter_range_returns_5() {
        let expr = r#"
              let y = 1:u64 + 10: u64;
              1..y
              "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiled = compiler::compile(expr, &vec![]).unwrap();

        let mut interpreter = Interpreter::default();
        let result = interpreter.run(compiled.byte_code).await.unwrap();

        let expected = ValueAndType::new(
            Value::Record(vec![Value::U64(1), Value::U64(11), Value::Bool(false)]),
            record(vec![
                field("from", option(u64())),
                field("to", option(u64())),
                field("inclusive", bool()),
            ]),
        );

        assert_eq!(result.get_val().unwrap(), expected);
    }

    #[test]
    async fn test_interpreter_range_with_comprehension_1() {
        let expr = r#"
              let range = 1..=5;
              for i in range {
                yield i;
              }

              "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiled = compiler::compile(expr, &vec![]).unwrap();

        let mut interpreter = Interpreter::default();
        let result = interpreter.run(compiled.byte_code).await.unwrap();

        let expected = ValueAndType::new(
            Value::List(vec![
                Value::U64(1),
                Value::U64(2),
                Value::U64(3),
                Value::U64(4),
                Value::U64(5),
            ]),
            list(u64()),
        );

        assert_eq!(result.get_val().unwrap(), expected);
    }

    #[test]
    async fn test_interpreter_range_with_comprehension_2() {
        let expr = r#"
              let range = 1..5;
              for i in range {
                yield i;
              }

              "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiled = compiler::compile(expr, &vec![]).unwrap();

        let mut interpreter = Interpreter::default();
        let result = interpreter.run(compiled.byte_code).await.unwrap();

        let expected = ValueAndType::new(
            Value::List(vec![
                Value::U64(1),
                Value::U64(2),
                Value::U64(3),
                Value::U64(4),
            ]),
            list(u64()),
        );

        assert_eq!(result.get_val().unwrap(), expected);
    }

    #[test]
    async fn test_interpreter_range_with_comprehension_3() {
        // infinite computation will respond with an error - than a stack overflow
        // Note that, `list[1..]` is allowed while `for i in 1.. { yield i; }` is not
        let expr = r#"
              let range = 1:u64..;
              for i in range {
                yield i;
              }

              "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiled = compiler::compile(expr, &vec![]).unwrap();

        let mut interpreter = Interpreter::default();
        let result = interpreter.run(compiled.byte_code).await;
        assert!(result.is_err());
    }

    #[test]
    async fn test_interpreter_range_with_list_reduce_1() {
        // infinite computation will respond with an error - than a stack overflow
        // Note that, `list[1..]` is allowed while `for i in 1.. { yield i; }` is not
        let expr = r#"
                let initial: u8 = 1;
                let final: u8 = 5;
                let x = initial..final;

                reduce z, a in x from 0u8 {
                  yield z + a;
                }

              "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiled = compiler::compile(expr, &vec![]).unwrap();

        let mut interpreter = Interpreter::default();
        let result = interpreter.run(compiled.byte_code).await.unwrap();

        let expected = ValueAndType::new(Value::U8(10), u8());

        assert_eq!(result.get_val().unwrap(), expected);
    }

    #[test]
    async fn test_interpreter_ephemeral_worker_0() {
        let expr = r#"
              let x = instance();
              let result = x.foo("bar");
              result
            "#;
        let expr = Expr::from_text(expr).unwrap();

        let component_metadata = test_utils::get_metadata();

        let compiled = compiler::compile(expr, &component_metadata).unwrap();

        let mut rib_interpreter = test_utils::interpreter_worker_details_response(None);

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        let expected_val = test_utils::parse_function_details(
            r#"
              {
                 worker-name: none,
                 function-name: "amazon:shopping-cart/api1.{foo}",
                 args0: "bar"
              }
            "#,
        );

        assert_eq!(result.get_val().unwrap(), expected_val);
    }

    #[test]
    async fn test_interpreter_ephemeral_worker_1() {
        let expr = r#"
              let x = instance();
              x
            "#;
        let expr = Expr::from_text(expr).unwrap();

        let component_metadata = test_utils::get_metadata();

        let compiled = compiler::compile(expr, &component_metadata);

        assert!(compiled.is_err());
    }

    #[test]
    async fn test_interpreter_ephemeral_worker_2() {
        let expr = r#"
             instance
            "#;
        let expr = Expr::from_text(expr).unwrap();

        let component_metadata = test_utils::get_metadata();

        let compiled = compiler::compile(expr, &component_metadata);

        assert!(compiled.is_err());
    }

    #[test]
    async fn test_interpreter_ephemeral_worker_3() {
        let expr = r#"
              instance()
            "#;
        let expr = Expr::from_text(expr).unwrap();

        let component_metadata = test_utils::get_metadata();

        let compiled = compiler::compile(expr, &component_metadata);

        assert!(compiled.is_err());
    }

    #[test]
    async fn test_interpreter_ephemeral_worker_4() {
        let expr = r#"
              instance().foo("bar")
            "#;
        let expr = Expr::from_text(expr).unwrap();

        let component_metadata = test_utils::get_metadata();

        let compiled = compiler::compile(expr, &component_metadata).unwrap();

        let mut rib_interpreter = test_utils::interpreter_worker_details_response(None);

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        let expected_val = test_utils::parse_function_details(
            r#"
              {
                 worker-name: none,
                 function-name: "amazon:shopping-cart/api1.{foo}",
                 args0: "bar"
              }
            "#,
        );

        assert_eq!(result.get_val().unwrap(), expected_val);
    }

    #[test]
    async fn test_interpreter_ephemeral_worker_5() {
        let expr = r#"
              let result = instance.foo("bar");
              result
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let component_metadata = test_utils::get_metadata();

        let compiled = compiler::compile(expr, &component_metadata)
            .unwrap_err()
            .to_string();

        assert_eq!(compiled, "error in the following rib found at line 2, column 28\n`instance`\ncause: `instance` is a reserved keyword\nhelp: use `instance()` instead of `instance` to create an ephemeral worker instance.\nhelp: for a durable worker, use `instance(\"foo\")` where `\"foo\"` is the worker name\n".to_string());
    }

    #[test]
    async fn test_interpreter_ephemeral_worker_6() {
        let expr = r#"
                let x = instance();
                let result = x.bar("bar");
                result
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let component_metadata = test_utils::get_metadata();

        let compilation_error = compiler::compile(expr, &component_metadata)
            .unwrap_err()
            .to_string();

        assert_eq!(
            compilation_error,
            "error in the following rib found at line 3, column 30\n`x.bar(\"bar\")`\ncause: invalid function call `bar`\nmultiple interfaces contain function 'bar'. specify an interface name as type parameter from: api1, api2\n".to_string()
        );
    }

    #[test]
    async fn test_interpreter_ephemeral_worker_7() {
        let expr = r#"
                let worker = instance();
                let invokes: list<u8> = [1, 2, 3, 4];

                for i in invokes {
                    yield worker.qux[wasi:clocks]("bar");
                };

                "success"
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let component_metadata = test_utils::get_metadata();

        let compiled = compiler::compile(expr, &component_metadata).unwrap();

        let mut rib_interpreter =
            test_utils::interpreter_static_response(&"success".into_value_and_type(), None);

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(result.get_val().unwrap(), "success".into_value_and_type());
    }

    /// Durable worker
    #[test]
    async fn test_interpreter_durable_worker_0() {
        let expr = r#"
                let worker = instance("my-worker");
                let result = worker.foo("bar");
                result
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let component_metadata = test_utils::get_metadata();

        let compiled = compiler::compile(expr, &component_metadata).unwrap();

        let mut rib_interpreter = test_utils::interpreter_worker_details_response(None);

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        let expected_val = test_utils::parse_function_details(
            r#"
              {
                 worker-name: some("my-worker"),
                 function-name: "amazon:shopping-cart/api1.{foo}",
                 args0: "bar"
              }
            "#,
        );

        assert_eq!(result.get_val().unwrap(), expected_val);
    }

    #[test]
    async fn test_interpreter_durable_worker_1() {
        let expr = r#"
                instance("my-worker").foo("bar")
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let component_metadata = test_utils::get_metadata();

        let compiled = compiler::compile(expr, &component_metadata).unwrap();

        let mut rib_interpreter = test_utils::interpreter_worker_details_response(None);

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        let expected_val = test_utils::parse_function_details(
            r#"
              {
                 worker-name: some("my-worker"),
                 function-name: "amazon:shopping-cart/api1.{foo}",
                 args0: "bar"
              }
            "#,
        );

        assert_eq!(result.get_val().unwrap(), expected_val);
    }

    #[test]
    async fn test_interpreter_durable_worker_2() {
        let expr = r#"
                let result = instance("my-worker").foo("bar");
                result
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let component_metadata = test_utils::get_metadata();

        let compiled = compiler::compile(expr, &component_metadata).unwrap();

        let mut rib_interpreter = test_utils::interpreter_worker_details_response(None);

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        let expected_val = test_utils::parse_function_details(
            r#"
              {
                 worker-name: some("my-worker"),
                 function-name: "amazon:shopping-cart/api1.{foo}",
                 args0: "bar"
              }
            "#,
        );

        assert_eq!(result.get_val().unwrap(), expected_val);
    }

    #[test]
    async fn test_interpreter_durable_worker_3() {
        let expr = r#"
                let my_worker = instance("my-worker");
                let result = my_worker.foo[api1]("bar");
                result
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let component_metadata = test_utils::get_metadata();

        let compiled = compiler::compile(expr, &component_metadata).unwrap();

        let mut rib_interpreter = test_utils::interpreter_worker_details_response(None);

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        let expected_val = test_utils::parse_function_details(
            r#"
              {
                 worker-name: some("my-worker"),
                 function-name: "amazon:shopping-cart/api1.{foo}",
                 args0: "bar"
              }
            "#,
        );

        assert_eq!(result.get_val().unwrap(), expected_val);
    }

    #[test]
    async fn test_interpreter_durable_worker_4() {
        let expr = r#"
                let worker = instance("my-worker");
                let result = worker.bar("bar");
                result
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let component_metadata = test_utils::get_metadata();

        let compilation_error = compiler::compile(expr, &component_metadata)
            .unwrap_err()
            .to_string();

        assert_eq!(
            compilation_error,
            "error in the following rib found at line 3, column 30\n`worker.bar(\"bar\")`\ncause: invalid function call `bar`\nmultiple interfaces contain function 'bar'. specify an interface name as type parameter from: api1, api2\n".to_string()
        );
    }

    #[test]
    async fn test_interpreter_durable_worker_5() {
        let expr = r#"
                let worker = instance("my-worker");
                let result = worker.bar[api1]("bar");
                result
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let component_metadata = test_utils::get_metadata();

        let compiled = compiler::compile(expr, &component_metadata).unwrap();

        let mut rib_interpreter = test_utils::interpreter_worker_details_response(None);

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        let expected_val = test_utils::parse_function_details(
            r#"
              {
                 worker-name: some("my-worker"),
                 function-name: "amazon:shopping-cart/api1.{bar}",
                 args0: "bar"
              }
            "#,
        );

        assert_eq!(result.get_val().unwrap(), expected_val);
    }

    #[test]
    async fn test_interpreter_durable_worker_6() {
        let expr = r#"
                let worker = instance("my-worker");
                let result = worker.bar[api2]("bar");
                result
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let component_metadata = test_utils::get_metadata();

        let compiled = compiler::compile(expr, &component_metadata).unwrap();

        let mut rib_interpreter = test_utils::interpreter_worker_details_response(None);

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        let expected_val = test_utils::parse_function_details(
            r#"
              {
                 worker-name: some("my-worker"),
                 function-name: "amazon:shopping-cart/api2.{bar}",
                 args0: "bar"
              }
            "#,
        );

        assert_eq!(result.get_val().unwrap(), expected_val);
    }

    #[test]
    async fn test_interpreter_durable_worker_7() {
        let expr = r#"
                let worker = instance("my-worker");
                let result = worker.baz("bar");
                result
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let component_metadata = test_utils::get_metadata();

        let compiled = compiler::compile(expr, &component_metadata).unwrap();

        let mut rib_interpreter = test_utils::interpreter_worker_details_response(None);

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        let expected_val = test_utils::parse_function_details(
            r#"
              {
                 worker-name: some("my-worker"),
                 function-name: "wasi:clocks/monotonic-clock.{baz}",
                 args0: "bar"
              }
            "#,
        );

        assert_eq!(result.get_val().unwrap(), expected_val);
    }

    #[test]
    async fn test_interpreter_durable_worker_8() {
        let expr = r#"
                let worker = instance("my-worker");
                let result = worker.qux("bar");
                result
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let component_metadata = test_utils::get_metadata();

        let compiled = compiler::compile(expr, &component_metadata)
            .unwrap_err()
            .to_string();

        assert_eq!(
            compiled,
            "error in the following rib found at line 3, column 30\n`worker.qux(\"bar\")`\ncause: invalid function call `qux`\nfunction 'qux' exists in multiple packages. specify a package name as type parameter from: amazon:shopping-cart (interfaces: api1), wasi:clocks (interfaces: monotonic-clock)\n".to_string()
        );
    }

    #[test]
    async fn test_interpreter_durable_worker_9() {
        let expr = r#"
                let worker = instance("my-worker");
                let result = worker.qux[amazon:shopping-cart]("bar");
                result
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let component_metadata = test_utils::get_metadata();

        let compiled = compiler::compile(expr, &component_metadata).unwrap();

        let mut rib_interpreter = test_utils::interpreter_worker_details_response(None);

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        let expected_val = test_utils::parse_function_details(
            r#"
              {
                 worker-name: some("my-worker"),
                 function-name: "amazon:shopping-cart/api1.{qux}",
                 args0: "bar"
              }
            "#,
        );

        assert_eq!(result.get_val().unwrap(), expected_val);
    }

    #[test]
    async fn test_interpreter_durable_worker_10() {
        let expr = r#"
                let worker = instance("my-worker");
                let result = worker.qux[wasi:clocks]("bar");
                result
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let component_metadata = test_utils::get_metadata();

        let compiled = compiler::compile(expr, &component_metadata).unwrap();

        let mut rib_interpreter = test_utils::interpreter_worker_details_response(None);

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        let expected_val = test_utils::parse_function_details(
            r#"
              {
                 worker-name: some("my-worker"),
                 function-name: "wasi:clocks/monotonic-clock.{qux}",
                 args0: "bar"
              }
            "#,
        );

        assert_eq!(result.get_val().unwrap(), expected_val);
    }

    #[test]
    async fn test_interpreter_durable_worker_11() {
        let expr = r#"
                let worker = instance("my-worker");
                let invokes: list<u8> = [1, 2, 3, 4];

                for i in invokes {
                    yield worker.qux[wasi:clocks]("bar");
                };

                "success"
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let component_metadata = test_utils::get_metadata();

        let compiled = compiler::compile(expr, &component_metadata).unwrap();

        let mut rib_interpreter =
            test_utils::interpreter_static_response(&"success".into_value_and_type(), None);

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(result.get_val().unwrap(), "success".into_value_and_type());
    }

    #[test]
    async fn test_interpreter_durable_worker_with_resource_0() {
        let expr = r#"
                let worker = instance("my-worker");
                worker.cart[golem:it]("bar")
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let component_metadata = test_utils::get_metadata_with_resource_with_params();

        let compiled = compiler::compile(expr, &component_metadata)
            .unwrap_err()
            .to_string();

        let expected = r#"
            error in the following rib found at line 3, column 17
            `cart("bar")`
            cause: program is invalid as it returns a resource constructor
            "#;

        assert_eq!(compiled, strip_spaces(expected));
    }

    // This resource construction is a Noop, and compiler can give warnings
    // once we support warnings in the compiler
    #[test]
    async fn test_interpreter_durable_worker_with_resource_1() {
        let expr = r#"
                let worker = instance("my-worker");
                worker.cart[golem:it]("bar");
                "success"
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let component_metadata = test_utils::get_metadata_with_resource_with_params();

        let compiled = compiler::compile(expr, &component_metadata).unwrap();

        let mut rib_interpreter =
            test_utils::interpreter_static_response(&"success".into_value_and_type(), None);

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(result.get_val().unwrap(), "success".into_value_and_type());
    }

    #[test]
    async fn test_interpreter_durable_worker_with_resource_2() {
        let expr = r#"
                let worker = instance("my-worker");
                let cart = worker.cart[golem:it]("bar");
                let result = cart.add-item({product-id: "mac", name: "macbook", quantity: 1:u32, price: 1:f32});
                result
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let component_metadata = test_utils::get_metadata_with_resource_with_params();

        let compiled = compiler::compile(expr, &component_metadata).unwrap();

        let mut rib_interpreter = test_utils::interpreter_worker_details_response(None);

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        let analysed_type = record(vec![
            field("worker-name", option(str())),
            field("function-name", str()),
            field(
                "args0",
                record(vec![
                    field("product-id", str()),
                    field("name", str()),
                    field("price", f32()),
                    field("quantity", u32()),
                ]),
            ),
        ]);

        let expected_val = get_value_and_type(
            &analysed_type,
            r#"
              {
                 worker-name: some("my-worker"),
                 function-name: "golem:it/api.{cart(\"bar\").add-item}",
                 args0: {product-id: "mac", name: "macbook", price: 1.0, quantity: 1}
              }
            "#,
        );

        assert_eq!(result.get_val().unwrap(), expected_val);
    }

    #[test]
    async fn test_interpreter_durable_worker_with_resource_3() {
        let expr = r#"
                let worker = instance("my-worker");
                let cart = worker.cart[golem:it]("bar");
                cart.add-items({product-id: "mac", name: "macbook", quantity: 1:u32, price: 1:f32});
                "success"
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let component_metadata = test_utils::get_metadata_with_resource_with_params();

        let compiled = compiler::compile(expr, &component_metadata)
            .unwrap_err()
            .to_string();

        assert_eq!(compiled, "error in the following rib found at line 4, column 17\n`cart.add-items({product-id: \"mac\", name: \"macbook\", quantity: 1: u32, price: 1: f32})`\ncause: invalid function call `add-items`\nfunction 'add-items' not found\n".to_string());
    }

    #[test]
    async fn test_interpreter_durable_worker_with_resource_4() {
        let expr = r#"
                let worker = instance("my-worker");
                let cart = worker.carts[golem:it]("bar");
                cart.add-item({product-id: "mac", name: "macbook", quantity: 1:u32, price: 1:f32});
                "success"
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let component_metadata = test_utils::get_metadata_with_resource_with_params();

        let compiled = compiler::compile(expr, &component_metadata)
            .unwrap_err()
            .to_string();

        assert_eq!(
            compiled,
            "error in the following rib found at line 3, column 28\n`worker.carts[golem:it](\"bar\")`\ncause: invalid function call `carts`\nfunction 'carts' not found in package 'golem:it'\n".to_string()
        );
    }

    #[test]
    async fn test_interpreter_durable_worker_with_resource_5() {
        // Ephemeral
        let expr = r#"
                let worker = instance();
                let cart = worker.cart[golem:it]("bar");
                cart.add-item({product-id: "mac", name: "macbook", quantity: 1, price: 1});
                "success"
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let component_metadata = test_utils::get_metadata_with_resource_with_params();

        let compiled = compiler::compile(expr, &component_metadata).unwrap();

        let mut rib_interpreter =
            test_utils::interpreter_static_response(&"success".into_value_and_type(), None);

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(result.get_val().unwrap(), "success".into_value_and_type());
    }

    #[test]
    async fn test_interpreter_durable_worker_with_resource_6() {
        // Ephemeral
        let expr = r#"
                let worker = instance();
                let cart = worker.cart[golem:it]("bar");
                cart.add-item({product-id: "mac", name: 1, quantity: 1, price: 1});
                "success"
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let component_metadata = test_utils::get_metadata_with_resource_with_params();

        let error_message = compiler::compile(expr, &component_metadata)
            .unwrap_err()
            .to_string();

        let expected = r#"
            error in the following rib found at line 4, column 31
            `{product-id: "mac", name: 1, quantity: 1, price: 1}`
            found within:
            `golem:it/api.{cart("bar").add-item}({product-id: "mac", name: 1, quantity: 1, price: 1})`
            cause: type mismatch at path: `name`. expected string
            invalid argument to the function `golem:it/api.{cart("bar").add-item}`
            "#;

        assert_eq!(error_message, strip_spaces(expected));
    }

    #[test]
    async fn test_interpreter_durable_worker_with_resource_7() {
        let expr = r#"
                let worker = instance("my-worker");
                let cart = worker.cart("bar");
                cart.add-item({product-id: "mac", name: "apple", price: 1, quantity: 1});
                "success"
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let component_metadata = test_utils::get_metadata_with_resource_with_params();

        let compiled = compiler::compile(expr, &component_metadata).unwrap();

        let mut rib_interpreter =
            test_utils::interpreter_static_response(&"success".into_value_and_type(), None);

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(result.get_val().unwrap(), "success".into_value_and_type());
    }

    #[test]
    async fn test_interpreter_durable_worker_with_resource_8() {
        let expr = r#"
                let worker = instance("my-worker");
                let a = "mac";
                let b = "apple";
                let c = 1;
                let d = 1;
                let cart = worker.cart("bar");
                cart.add-item({product-id: a, name: b, quantity: c, price: d});
                "success"
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let component_metadata = test_utils::get_metadata_with_resource_with_params();

        let compiled = compiler::compile(expr, &component_metadata).unwrap();

        let mut rib_interpreter =
            test_utils::interpreter_static_response(&"success".into_value_and_type(), None);

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(result.get_val().unwrap(), "success".into_value_and_type());
    }

    #[test]
    async fn test_interpreter_durable_worker_with_resource_9() {
        let expr = r#"
                let worker = instance("my-worker");
                let a = "mac";
                let b = "apple";
                let c = 1;
                let d = 1;
                let cart = worker.cart("bar");
                cart.add-item({product-id: a, name: b, quantity: c, price: d});
                cart.remove-item(a);
                cart.update-item-quantity(a, 2);
                let result = cart.get-cart-contents();
                cart.drop();
                result
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let component_metadata = test_utils::get_metadata_with_resource_with_params();

        let compiled = compiler::compile(expr, &component_metadata).unwrap();

        let mut rib_interpreter =
            test_utils::interpreter_static_response(&"success".into_value_and_type(), None);

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(result.get_val().unwrap(), "success".into_value_and_type());
    }

    #[test]
    async fn test_interpreter_durable_worker_with_resource_10() {
        let expr = r#"
                let my_worker = "my-worker";
                let worker = instance(my_worker);
                let a = "mac";
                let b = "apple";
                let c = 1;
                let d = 1;
                let cart = worker.cart("bar");
                cart.add-item({product-id: a, name: b, price: d, quantity: c});
                cart.remove-item(a);
                cart.update-item-quantity(a, 2);
                let result = cart.get-cart-contents();
                cart.drop();
                result
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let component_metadata = test_utils::get_metadata_with_resource_with_params();

        let compiled = compiler::compile(expr, &component_metadata).unwrap();

        let mut rib_interpreter =
            test_utils::interpreter_static_response(&"success".into_value_and_type(), None);

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(result.get_val().unwrap(), "success".into_value_and_type());
    }

    #[test]
    async fn test_interpreter_durable_worker_with_resource_11() {
        let expr = r#"
                let worker = instance(request.path.user-id: string);
                let result = worker.qux[amazon:shopping-cart]("bar");
                result
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let component_metadata = test_utils::get_metadata();

        let compiled = compiler::compile(expr, &component_metadata).unwrap();

        let mut input = HashMap::new();

        // Passing request data as input to interpreter
        let rib_input_key = "request";
        let rib_input_value = ValueAndType::new(
            Value::Record(vec![Value::Record(vec![Value::String("user".to_string())])]),
            record(vec![field("path", record(vec![field("user-id", str())]))]),
        );

        input.insert(rib_input_key.to_string(), rib_input_value);

        let rib_input = RibInput::new(input);

        let mut rib_interpreter = test_utils::interpreter_static_response(
            &"success".into_value_and_type(),
            Some(rib_input),
        );

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(result.get_val().unwrap(), "success".into_value_and_type());
    }

    #[test]
    async fn test_interpreter_durable_worker_with_resource_12() {
        let expr = r#"
                let user_id1: string = request.path.user-id;
                let user_id2: string = request.path.user-id;
                let worker1 = instance(user_id1);
                let result1 = worker1.qux[amazon:shopping-cart]("bar");
                let worker2 = instance(user_id2);
                let result2 = worker2.qux[amazon:shopping-cart]("bar");
                user_id2
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let component_metadata = test_utils::get_metadata();

        let compiled = compiler::compile(expr, &component_metadata).unwrap();

        let mut input = HashMap::new();

        let rib_input_key = "request";
        let rib_input_value = ValueAndType::new(
            Value::Record(vec![Value::Record(vec![Value::String("user".to_string())])]),
            record(vec![field("path", record(vec![field("user-id", str())]))]),
        );

        input.insert(rib_input_key.to_string(), rib_input_value);

        let rib_input = RibInput::new(input);

        let mut rib_interpreter = test_utils::interpreter_static_response(
            &"success".into_value_and_type(),
            Some(rib_input),
        );

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(result.get_val().unwrap(), "user".into_value_and_type());
    }

    #[test]
    async fn test_interpreter_durable_worker_with_resource_13() {
        let expr = r#"
                let worker1 = instance("foo");
                let result = worker.qux[amazon:shopping-cart]("bar");
                "success"
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let component_metadata = test_utils::get_metadata();

        let error = compiler::compile(expr, &component_metadata)
            .unwrap_err()
            .to_string();

        assert_eq!(error, "error in the following rib found at line 3, column 30\n`worker.qux[amazon:shopping-cart](\"bar\")`\ncause: invalid method invocation `worker.qux`. make sure `worker` is defined and is a valid instance type (i.e, resource or worker)\n");
    }

    #[test]
    async fn test_interpreter_durable_worker_with_resource_14() {
        let expr = r#"
                let worker = instance(1: u32);
                let result = worker.qux[amazon:shopping-cart]("bar");
                "success"
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let component_metadata = test_utils::get_metadata();

        let error = compiler::compile(expr, &component_metadata)
            .unwrap_err()
            .to_string();

        let expected = r#"
            error in the following rib found at line 2, column 39
            `1: u32`
            cause: expected string, found u32
            "#;

        assert_eq!(error, strip_spaces(expected));
    }

    #[test]
    async fn test_interpreter_durable_worker_with_resource_15() {
        let expr = r#"
                let worker = instance("my-worker-name");
                let result = worker.qux[amazon:shopping-cart]("param1");
                result
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let component_metadata = test_utils::get_metadata();

        let compiled = compiler::compile(expr, &component_metadata).unwrap();

        let mut rib_interpreter = test_utils::interpreter_worker_details_response(None);

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        let result_val = result.get_val().unwrap();

        let expected_val = test_utils::parse_function_details(
            r#"
              {
                 worker-name: some("my-worker-name"),
                 function-name: "amazon:shopping-cart/api1.{qux}",
                 args0: "param1"
              }
            "#,
        );

        assert_eq!(result_val, expected_val);
    }

    #[test]
    async fn test_interpreter_durable_worker_with_resource_16() {
        let expr = r#"
                let x = request.path.user-id;
                let worker = instance(x);
                let cart = worker.cart("bar");
                let result = cart.get-cart-contents();
                result
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let component_metadata = test_utils::get_metadata_with_resource_with_params();

        let compiled = compiler::compile(expr, &component_metadata).unwrap();

        let mut input = HashMap::new();

        let rib_input_key = "request";
        let rib_input_value = ValueAndType::new(
            Value::Record(vec![Value::Record(vec![Value::String("user".to_string())])]),
            record(vec![field("path", record(vec![field("user-id", str())]))]),
        );

        input.insert(rib_input_key.to_string(), rib_input_value);

        let rib_input = RibInput::new(input);

        let mut rib_interpreter = test_utils::interpreter_worker_details_response(Some(rib_input));

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        let result_val = result.get_val().unwrap();

        let expected_analysed_type = record(vec![
            field("worker-name", option(str())),
            field("function-name", str()),
        ]);

        let expected_val = parse_value_and_type(
            &expected_analysed_type,
            r#"
              {
                 worker-name: some("user"),
                 function-name: "golem:it/api.{cart(\"bar\").get-cart-contents}",
              }
            "#,
        )
        .unwrap();

        assert_eq!(result_val, expected_val)
    }

    #[test]
    async fn test_interpreter_durable_worker_with_resource_17() {
        let expr = r#"
                let x: string = request.path.user-id;
                let min: u8 = 1;
                let max: u8 = 3;
                let result = for i in min..=max {
                   let worker = instance("my-worker");
                   let cart = worker.cart("bar");
                   yield cart.get-cart-contents();
                };
                result
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let component_metadata = test_utils::get_metadata_with_resource_with_params();

        let compiled = compiler::compile(expr, &component_metadata).unwrap();

        let mut input = HashMap::new();

        let rib_input_key = "request";
        let rib_input_value = ValueAndType::new(
            Value::Record(vec![Value::Record(vec![Value::String("user".to_string())])]),
            record(vec![field("path", record(vec![field("user-id", str())]))]),
        );

        input.insert(rib_input_key.to_string(), rib_input_value);

        let rib_input = RibInput::new(input);

        let mut rib_interpreter = test_utils::interpreter_worker_details_response(Some(rib_input));

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        let result_val = result.get_val().unwrap().value;

        let worker_name = Some("my-worker".to_string()).into_value();
        let function_name = "golem:it/api.{cart(\"bar\").get-cart-contents}"
            .to_string()
            .into_value();

        let expected = Value::List(vec![
            Value::Record(vec![worker_name.clone(), function_name.clone()]),
            Value::Record(vec![worker_name.clone(), function_name.clone()]),
            Value::Record(vec![worker_name.clone(), function_name.clone()]),
        ]);

        assert_eq!(result_val, expected);
    }

    #[test]
    async fn test_interpreter_durable_worker_with_resource_18() {
        let expr = r#"

            let initial = 1: u64;
            let final = 5: u64;
            let range = initial..final;
            let worker = instance("my-worker");
            let cart = worker.cart[golem:it]("bar");

            for i in range {
                yield cart.add-item(request.body);
            };

            "success"
        "#;
        let expr = Expr::from_text(expr).unwrap();
        let component_metadata = test_utils::get_metadata_with_resource_with_params();

        let compiled = compiler::compile(expr, &component_metadata).unwrap();

        let mut input = HashMap::new();

        let rib_input_key = "request";
        let rib_input_value = ValueAndType::new(
            Value::Record(vec![Value::Record(vec![
                Value::String("mac-book".to_string()),
                Value::String("mac".to_string()),
                Value::U32(1),
                Value::F32(1.0),
            ])]),
            record(vec![field(
                "body",
                record(vec![
                    field("name", str()),
                    field("product-id", str()),
                    field("quantity", u32()),
                    field("price", f32()),
                ]),
            )]),
        );

        input.insert(rib_input_key.to_string(), rib_input_value);

        let rib_input = RibInput::new(input);

        let mut rib_interpreter = test_utils::interpreter_static_response(
            &"success".into_value_and_type(),
            Some(rib_input),
        );

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(result.get_val().unwrap(), "success".into_value_and_type());
    }

    #[test]
    async fn test_interpreter_durable_worker_with_resource_19() {
        let expr = r#"

            let initial = 1: u64;
            let final = 5: u64;
            let range = initial..final;

            for i in range {
                let worker = instance("my-worker");
                let cart = worker.cart[golem:it]("bar");
                yield cart.add-item(request.body);
            };

            "success"
        "#;
        let expr = Expr::from_text(expr).unwrap();
        let component_metadata = test_utils::get_metadata_with_resource_with_params();

        let compiled = compiler::compile(expr, &component_metadata).unwrap();

        let mut input = HashMap::new();

        let rib_input_key = "request";
        let rib_input_value = ValueAndType::new(
            Value::Record(vec![Value::Record(vec![
                Value::String("mac-book".to_string()),
                Value::String("mac".to_string()),
                Value::U32(1),
                Value::F32(1.0),
            ])]),
            record(vec![field(
                "body",
                record(vec![
                    field("name", str()),
                    field("product-id", str()),
                    field("quantity", u32()),
                    field("price", f32()),
                ]),
            )]),
        );

        input.insert(rib_input_key.to_string(), rib_input_value);

        let rib_input = RibInput::new(input);

        let mut rib_interpreter = test_utils::interpreter_static_response(
            &"success".into_value_and_type(),
            Some(rib_input),
        );

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(result.get_val().unwrap(), "success".into_value_and_type());
    }

    mod test_utils {
        use crate::interpreter::rib_interpreter::Interpreter;
        use crate::{
            EvaluatedFnArgs, EvaluatedFqFn, EvaluatedWorkerName, GetLiteralValue,
            RibFunctionInvoke, RibInput,
        };
        use async_trait::async_trait;
        use golem_wasm_ast::analysis::analysed_type::{
            case, f32, field, handle, list, option, r#enum, record, result, str, tuple, u32, u64,
            unit_case, variant,
        };
        use golem_wasm_ast::analysis::{
            AnalysedExport, AnalysedFunction, AnalysedFunctionParameter, AnalysedFunctionResult,
            AnalysedInstance, AnalysedResourceId, AnalysedResourceMode, AnalysedType,
        };
        use golem_wasm_rpc::{IntoValueAndType, Value, ValueAndType};
        use std::sync::Arc;

        pub(crate) fn strip_spaces(input: &str) -> String {
            let lines = input.lines();

            let first_line = lines
                .clone()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("");
            let margin_width = first_line.chars().take_while(|c| c.is_whitespace()).count();

            let result = lines
                .map(|line| {
                    if line.trim().is_empty() {
                        String::new()
                    } else {
                        line[margin_width..].to_string()
                    }
                })
                .collect::<Vec<String>>()
                .join("\n");

            result.strip_prefix("\n").unwrap_or(&result).to_string()
        }

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

        pub(crate) fn get_metadata_with_resource_with_params() -> Vec<AnalysedExport> {
            get_metadata_with_resource(vec![AnalysedFunctionParameter {
                name: "user-id".to_string(),
                typ: str(),
            }])
        }

        pub(crate) fn get_metadata_with_resource_without_params() -> Vec<AnalysedExport> {
            get_metadata_with_resource(vec![])
        }

        pub(crate) fn get_metadata() -> Vec<AnalysedExport> {
            // Exist in only amazon:shopping-cart/api1
            let analysed_function_in_api1 = AnalysedFunction {
                name: "foo".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "arg1".to_string(),
                    typ: str(),
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: str(),
                }],
            };

            // Exist in both amazon:shopping-cart/api1 and amazon:shopping-cart/api2
            let analysed_function_in_api1_and_api2 = AnalysedFunction {
                name: "bar".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "arg1".to_string(),
                    typ: str(),
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: str(),
                }],
            };

            // Exist in only wasi:clocks/monotonic-clock
            let analysed_function_in_wasi = AnalysedFunction {
                name: "baz".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "arg1".to_string(),
                    typ: str(),
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: str(),
                }],
            };

            // Exist in wasi:clocks/monotonic-clock and amazon:shopping-cart/api1
            let analysed_function_in_wasi_and_api1 = AnalysedFunction {
                name: "qux".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "arg1".to_string(),
                    typ: str(),
                }],
                results: vec![AnalysedFunctionResult {
                    name: None,
                    typ: str(),
                }],
            };

            let analysed_export1 = AnalysedExport::Instance(AnalysedInstance {
                name: "amazon:shopping-cart/api1".to_string(),
                functions: vec![
                    analysed_function_in_api1,
                    analysed_function_in_api1_and_api2.clone(),
                    analysed_function_in_wasi_and_api1.clone(),
                ],
            });

            let analysed_export2 = AnalysedExport::Instance(AnalysedInstance {
                name: "amazon:shopping-cart/api2".to_string(),
                functions: vec![analysed_function_in_api1_and_api2],
            });

            let analysed_export3 = AnalysedExport::Instance(AnalysedInstance {
                name: "wasi:clocks/monotonic-clock".to_string(),
                functions: vec![
                    analysed_function_in_wasi,
                    analysed_function_in_wasi_and_api1,
                ],
            });

            vec![analysed_export1, analysed_export2, analysed_export3]
        }

        fn get_metadata_with_resource(
            resource_constructor_params: Vec<AnalysedFunctionParameter>,
        ) -> Vec<AnalysedExport> {
            let instance = AnalysedExport::Instance(AnalysedInstance {
                name: "golem:it/api".to_string(),
                functions: vec![
                    AnalysedFunction {
                        name: "[constructor]cart".to_string(),
                        parameters: resource_constructor_params,
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

        pub(crate) fn get_value_and_type(
            analysed_type: &AnalysedType,
            wasm_wave_str: &str,
        ) -> ValueAndType {
            golem_wasm_rpc::parse_value_and_type(analysed_type, wasm_wave_str).unwrap()
        }

        // The interpreter that always returns a static value for every function calls in Rib
        // regardless of the input arguments
        pub(crate) fn interpreter_static_response(
            result_value: &ValueAndType,
            input: Option<RibInput>,
        ) -> Interpreter {
            let value = result_value.clone();

            let invoke = Arc::new(TestInvoke1 { value });

            Interpreter {
                input: input.unwrap_or_default(),
                invoke,
                custom_stack: None,
                custom_env: None,
            }
        }

        // The interpreter that always returns a record value consisting of function name, worker name etc
        // for every function calls in Rib.
        // Example : `my-instance.qux[amazon:shopping-cart]("bar")` will return a record
        // that contains the actual worker-name of my-instance, the function name `qux` and arguments
        // It helps ensures that interpreter invokes the function at the expected worker.
        pub(crate) fn interpreter_worker_details_response(
            rib_input: Option<RibInput>,
        ) -> Interpreter {
            let invoke: Arc<dyn RibFunctionInvoke + Send + Sync> = Arc::new(TestInvoke2);

            Interpreter {
                input: rib_input.unwrap_or_default(),
                invoke,
                custom_stack: None,
                custom_env: None,
            }
        }

        // A simple interpreter that returns response based on the function
        pub(crate) fn interpreter_dynamic_response(input: Option<RibInput>) -> Interpreter {
            let invoke = Arc::new(TestInvoke3);

            Interpreter {
                input: input.unwrap_or_default(),
                invoke,
                custom_stack: None,
                custom_env: None,
            }
        }

        pub(crate) fn parse_function_details(input: &str) -> ValueAndType {
            let analysed_type = record(vec![
                field("worker-name", option(str())),
                field("function-name", str()),
                field("args0", str()),
            ]);

            get_value_and_type(&analysed_type, input)
        }

        struct TestInvoke1 {
            value: ValueAndType,
        }

        #[async_trait]
        impl RibFunctionInvoke for TestInvoke1 {
            async fn invoke(
                &self,
                _worker_name: Option<EvaluatedWorkerName>,
                _fqn: EvaluatedFqFn,
                _args: EvaluatedFnArgs,
            ) -> Result<ValueAndType, String> {
                let value = self.value.clone();
                Ok(ValueAndType::new(
                    Value::Tuple(vec![value.value]),
                    tuple(vec![value.typ]),
                ))
            }
        }

        struct TestInvoke2;

        #[async_trait]
        impl RibFunctionInvoke for TestInvoke2 {
            async fn invoke(
                &self,
                worker_name: Option<EvaluatedWorkerName>,
                function_name: EvaluatedFqFn,
                args: EvaluatedFnArgs,
            ) -> Result<ValueAndType, String> {
                let worker_name = worker_name.map(|x| x.0);

                let function_name = function_name.0.into_value_and_type();

                let args = args.0;

                let mut arg_types = vec![];

                for (index, value_and_type) in args.iter().enumerate() {
                    let name = format!("args{}", index);
                    let value = value_and_type.typ.clone();
                    arg_types.push(field(name.as_str(), value));
                }

                let mut analysed_type_pairs = vec![];
                analysed_type_pairs.push(field("worker-name", option(str())));
                analysed_type_pairs.push(field("function-name", str()));
                analysed_type_pairs.extend(arg_types);

                let mut values = vec![];

                values.push(Value::Option(
                    worker_name.map(|x| Box::new(Value::String(x))),
                ));
                values.push(function_name.value);

                for arg_value in args {
                    values.push(arg_value.value);
                }

                let value = ValueAndType::new(
                    Value::Tuple(vec![Value::Record(values)]),
                    tuple(vec![record(analysed_type_pairs)]),
                );
                Ok(value)
            }
        }

        pub(crate) fn get_metadata_with_enum_and_variant() -> Vec<AnalysedExport> {
            vec![
                AnalysedExport::Function(AnalysedFunction {
                    name: "add-u32".to_string(),
                    parameters: vec![
                        AnalysedFunctionParameter {
                            name: "param1".to_string(),
                            typ: u32(),
                        },
                        AnalysedFunctionParameter {
                            name: "param2".to_string(),
                            typ: u32(),
                        },
                    ],
                    results: vec![AnalysedFunctionResult {
                        name: None,
                        typ: u32(),
                    }],
                }),
                AnalysedExport::Function(AnalysedFunction {
                    name: "add-u64".to_string(),
                    parameters: vec![
                        AnalysedFunctionParameter {
                            name: "param1".to_string(),
                            typ: u64(),
                        },
                        AnalysedFunctionParameter {
                            name: "param2".to_string(),
                            typ: u64(),
                        },
                    ],
                    results: vec![AnalysedFunctionResult {
                        name: None,
                        typ: u64(),
                    }],
                }),
                AnalysedExport::Function(AnalysedFunction {
                    name: "add-enum".to_string(),
                    parameters: vec![
                        AnalysedFunctionParameter {
                            name: "param1".to_string(),
                            typ: r#enum(&["x", "y", "z"]),
                        },
                        AnalysedFunctionParameter {
                            name: "param2".to_string(),
                            typ: r#enum(&["x", "y", "z"]),
                        },
                    ],
                    results: vec![AnalysedFunctionResult {
                        name: None,
                        typ: r#enum(&["x", "y", "z"]),
                    }],
                }),
                AnalysedExport::Function(AnalysedFunction {
                    name: "add-variant".to_string(),
                    parameters: vec![
                        AnalysedFunctionParameter {
                            name: "param1".to_string(),
                            typ: get_analysed_type_variant(),
                        },
                        AnalysedFunctionParameter {
                            name: "param2".to_string(),
                            typ: get_analysed_type_variant(),
                        },
                    ],
                    results: vec![AnalysedFunctionResult {
                        name: None,
                        typ: get_analysed_type_variant(),
                    }],
                }),
            ]
        }

        struct TestInvoke3;

        #[async_trait]
        impl RibFunctionInvoke for TestInvoke3 {
            async fn invoke(
                &self,
                _worker_name: Option<EvaluatedWorkerName>,
                function_name: EvaluatedFqFn,
                args: EvaluatedFnArgs,
            ) -> Result<ValueAndType, String> {
                match function_name.0.as_str() {
                    "add-u32" => {
                        let args = args.0;
                        let arg1 = args[0].get_literal().and_then(|x| x.get_number()).unwrap();
                        let arg2 = args[1].get_literal().and_then(|x| x.get_number()).unwrap();
                        let result = arg1 + arg2;
                        let u32 = result.cast_to(&u32()).unwrap();

                        Ok(ValueAndType::new(
                            Value::Tuple(vec![u32.value]),
                            tuple(vec![u32.typ]),
                        ))
                    }
                    "add-u64" => {
                        let args = args.0;
                        let arg1 = args[0].get_literal().and_then(|x| x.get_number()).unwrap();
                        let arg2 = args[1].get_literal().and_then(|x| x.get_number()).unwrap();
                        let result = arg1 + arg2;
                        let u64 = result.cast_to(&u64()).unwrap();
                        Ok(ValueAndType::new(
                            Value::Tuple(vec![u64.value]),
                            tuple(vec![u64.typ]),
                        ))
                    }
                    "add-enum" => {
                        let args = args.0;
                        let arg1 = args[0].clone().value;
                        let arg2 = args[1].clone().value;
                        match (arg1, arg2) {
                            (Value::Enum(x), Value::Enum(y)) => {
                                if x == y {
                                    let result =
                                        ValueAndType::new(Value::Enum(x), r#enum(&["x", "y", "z"]));
                                    Ok(ValueAndType::new(
                                        Value::Tuple(vec![result.value]),
                                        tuple(vec![result.typ]),
                                    ))
                                } else {
                                    Err(format!("Enums are not equal: {} and {}", x, y))
                                }
                            }
                            (v1, v2) => Err(format!(
                                "Invalid arguments for add-enum: {:?} and {:?}",
                                v1, v2
                            )),
                        }
                    }
                    "add-variant" => {
                        let args = args.0;
                        let arg1 = args[0].clone().value;
                        let arg2 = args[1].clone().value;
                        match (arg1, arg2) {
                            (
                                Value::Variant {
                                    case_idx: case_idx1,
                                    case_value,
                                },
                                Value::Variant {
                                    case_idx: case_idx2,
                                    ..
                                },
                            ) => {
                                if case_idx1 == case_idx2 {
                                    let result = ValueAndType::new(
                                        Value::Variant {
                                            case_idx: case_idx1,
                                            case_value,
                                        },
                                        get_analysed_type_variant(),
                                    );
                                    Ok(ValueAndType::new(
                                        Value::Tuple(vec![result.value]),
                                        tuple(vec![result.typ]),
                                    ))
                                } else {
                                    Err(format!(
                                        "Variants are not equal: {} and {}",
                                        case_idx1, case_idx2
                                    ))
                                }
                            }
                            (v1, v2) => Err(format!(
                                "Invalid arguments for add-variant: {:?} and {:?}",
                                v1, v2
                            )),
                        }
                    }
                    fun => Err(format!("unknown function {}", fun)),
                }
            }
        }
    }
}
