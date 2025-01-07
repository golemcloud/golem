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
                RibIR::Plus(analysed_type) => {
                    internal::run_math_instruction(
                        &mut stack,
                        |left, right| left + right,
                        &analysed_type,
                    )?;
                }
                RibIR::Minus(analysed_type) => {
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
        CoercedNumericValue, FunctionReferenceType, InstructionId, ParsedFunctionName,
        ParsedFunctionReference, ParsedFunctionSite, RibFunctionInvoke, VariableId,
    };
    use golem_wasm_ast::analysis::AnalysedType;
    use golem_wasm_ast::analysis::TypeResult;
    use golem_wasm_rpc::{print_value_and_type, IntoValueAndType, Value, ValueAndType};

    use crate::interpreter::instruction_cursor::RibByteCodeCursor;
    use golem_wasm_ast::analysis::analysed_type::{str, tuple};
    use std::ops::Deref;
    use std::sync::Arc;

    pub(crate) fn default_worker_invoke_async() -> RibFunctionInvoke {
        Arc::new(|_, _| {
            Box::pin(async {
                Ok(ValueAndType {
                    value: Value::Tuple(vec![]),
                    typ: tuple(vec![]),
                })
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
        if let Some(items) = interpreter_stack
            .pop()
            .and_then(|v| v.get_val())
            .and_then(|v| v.into_list_items())
        {
            interpreter_stack.push(RibInterpreterStackValue::Iterator(Box::new(
                items.into_iter(),
            )));

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
        interpreter_stack.push_list(result.into_iter().map(|vnt| vnt.value).collect(), &str());

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
            AnalysedType::Record(type_record) => type_record.fields,
            _ => {
                return Err(format!(
                    "Internal Error: Expected a record type to create a record. But obtained {:?}",
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
            .ok_or(format!(
                "Invalid field name {field_name}, should be one of {}",
                record_type
                    .fields
                    .iter()
                    .map(|pair| pair.name.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))?;
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

            _ => Err(format!("Internal Error: Failed to create tuple due to mismatch in types. Expected: list, Actual: {:?}", analysed_type)),
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

            _ => Err(format!("Internal Error: Failed to create tuple due to mismatch in types. Expected: tuple, Actual: {:?}", analysed_type)),
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
        let left = interpreter_stack.try_pop()?;
        let right = interpreter_stack.try_pop()?;

        let result = left.compare(&right, compare_fn)?;

        interpreter_stack.push(result);

        Ok(())
    }

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
                    .ok_or(format!("Field {} not found in the record", field_name))?;

                let value = field.0;
                interpreter_stack.push_val(ValueAndType::new(value, field.1.typ));
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
            RibInterpreterStackValue::Val(ValueAndType {
                value: Value::List(items),
                typ: AnalysedType::List(typ),
            }) => {
                let value = items
                    .get(index)
                    .ok_or(format!("Index {} not found in the list", index))?
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
                    .ok_or(format!("Index {} not found in the tuple", index))?
                    .clone();

                let item_type = typ
                    .items
                    .get(index)
                    .ok_or(format!("Index {} not found in the tuple type", index))?
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

                let arg_value = match variant_arg_typ {
                    Some(_) => Some(interpreter_stack.try_pop_val()?),
                    None => None,
                };

                interpreter_stack.push_variant(
                    variant_name.clone(),
                    arg_value.map(|vnt| vnt.value),
                    variants.cases.clone(),
                );
                Ok(())
            }

            _ => Err(format!(
                "Internal Error: Expected a variant type for {}, but obtained {:?}",
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
                    .ok_or("Failed to get values from the stack".to_string())?;

                let parameter_values = last_n_elements
                    .iter()
                    .map(|interpreter_result| {
                        interpreter_result
                            .get_val()
                            .ok_or("Internal Error: Failed to construct resource".to_string())
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
                    .ok_or("Failed to get values from the stack".to_string())?;

                let param_values = last_n_elements
                    .iter()
                    .map(|interpreter_result| {
                        interpreter_result.get_val().ok_or(
                            "Internal Error: Failed to call indexed resource method".to_string(),
                        )
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
                let last_n_elements = interpreter_stack.pop_n(arg_size).ok_or(
                    "Internal error: Failed to get arguments for static resource method"
                        .to_string(),
                )?;

                let param_values = last_n_elements
                    .iter()
                    .map(|interpreter_result| {
                        interpreter_result.get_val().ok_or(
                            "Internal error: Failed to call static resource method".to_string(),
                        )
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
                let last_n_elements = interpreter_stack.pop_n(arg_size).ok_or(
                    "Internal Error: Failed to get resource parameters for indexed resource drop"
                        .to_string(),
                )?;

                let param_values = last_n_elements
                    .iter()
                    .map(|interpreter_result| {
                        interpreter_result.get_val().ok_or(
                            "Internal Error: Failed to call indexed resource drop".to_string(),
                        )
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
        interpreter_stack: &mut InterpreterStack,
        interpreter_env: &mut InterpreterEnv,
    ) -> Result<(), String> {
        let function_name = interpreter_stack
            .pop_str()
            .ok_or("Internal Error: Failed to get a function name".to_string())?;

        let last_n_elements = interpreter_stack
            .pop_n(arg_size)
            .ok_or("Internal Error: Failed to get arguments for the function call".to_string())?;

        let parameter_values = last_n_elements
            .iter()
            .map(|interpreter_result| {
                interpreter_result.get_val().ok_or(format!(
                    "Internal Error: Failed to call function {}",
                    function_name
                ))
            })
            .collect::<Result<Vec<ValueAndType>, String>>()?;

        let result = interpreter_env
            .invoke_worker_function_async(function_name, parameter_values)
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
                "Internal Error: Expected option type to create `some` value. But obtained {:?}",
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
                "Internal Error: Expected option type to create `none` value. But obtained {:?}",
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
                "Internal Error: Expected result type to create `ok` value. But obtained {:?}",
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
                "Internal Error: Expected result type to create `err` value. But obtained {:?}",
                analysed_type
            )),
        }
    }

    pub(crate) fn run_concat_instruction(
        interpreter_stack: &mut InterpreterStack,
        arg_size: usize,
    ) -> Result<(), String> {
        let literals = interpreter_stack.try_pop_n_literals(arg_size)?;

        let str = literals
            .into_iter()
            .fold(String::new(), |mut acc, literal| {
                acc.push_str(&literal.as_string());
                acc
            });

        interpreter_stack.push_val(str.into_value_and_type());

        Ok(())
    }
}

#[cfg(test)]
mod interpreter_tests {
    use test_r::test;

    use super::*;
    use crate::{InstructionId, VariableId};
    use golem_wasm_ast::analysis::analysed_type::{field, list, record, s32};
    use golem_wasm_rpc::{IntoValue, IntoValueAndType, Value, ValueAndType};

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
        assert!(result.is_err());
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
        assert!(result.is_err());
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

    mod list_reduce_interpreter_tests {
        use crate::interpreter::rib_interpreter::Interpreter;
        use crate::{compiler, Expr};
        use golem_wasm_rpc::IntoValueAndType;
        use test_r::test;

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

            assert_eq!(result, 3u8.into_value_and_type());
        }

        #[test]
        async fn test_list_reduce_from_record() {
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

            let compiled = compiler::compile(&expr, &vec![]).unwrap();

            let result = interpreter
                .run(compiled.byte_code)
                .await
                .unwrap()
                .get_val()
                .unwrap();

            assert_eq!(result, "foo, bar".into_value_and_type());
        }

        #[test]
        async fn test_list_reduce_text() {
            let mut interpreter = Interpreter::default();

            let rib_expr = r#"
           let x = ["foo", "bar"];

          reduce z, a in x from "" {
            let result = if z == "" then a else "${z}, ${a}";

            yield result;
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

            assert_eq!(result, "foo, bar".into_value_and_type());
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

            assert_eq!(result, 0u8.into_value_and_type());
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
            let expected_value =
                golem_wasm_rpc::parse_value_and_type(&list(str()), expected).unwrap();

            assert_eq!(result, expected_value);
        }

        #[test]
        async fn test_list_comprehension_empty() {
            let mut interpreter = Interpreter::default();

            let rib_expr = r#"
          let x: list<string> = [];

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
                golem_wasm_rpc::parse_value_and_type(&list(str()), expected).unwrap();

            assert_eq!(result, expected_type_annotated_value);
        }
    }

    mod pattern_match_interpreter_tests {
        use test_r::test;

        use crate::interpreter::rib_interpreter::interpreter_tests::internal;
        use crate::interpreter::rib_interpreter::Interpreter;
        use crate::{compiler, Expr, FunctionTypeRegistry};
        use golem_wasm_ast::analysis::analysed_type::{field, record, str, tuple, u16, u64};
        use golem_wasm_rpc::IntoValueAndType;

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

            assert_eq!(result.get_val().unwrap(), 0u64.into_value_and_type());
        }

        #[test]
        async fn test_pattern_match_on_tuple() {
            let mut interpreter = Interpreter::default();

            let expr = r#"
           let x: tuple<u64, string, string> = (1, "foo", "bar");

           match x {
              (x, y, z) => "${x} ${y} ${z}"
           }
        "#;

            let mut expr = Expr::from_text(expr).unwrap();
            expr.infer_types(&FunctionTypeRegistry::empty()).unwrap();
            let compiled = compiler::compile(&expr, &vec![]).unwrap();
            let result = interpreter.run(compiled.byte_code).await.unwrap();

            assert_eq!(result.get_val().unwrap(), "1 foo bar".into_value_and_type());
        }

        #[test]
        async fn test_pattern_match_on_tuple_with_option_some() {
            let mut interpreter = Interpreter::default();

            let expr = r#"
           let x: tuple<u64, option<string>, string> = (1, some("foo"), "bar");

           match x {
              (x, none, z) => "${x} ${z}",
              (x, some(y), z) => "${x} ${y} ${z}"
           }
        "#;

            let mut expr = Expr::from_text(expr).unwrap();
            expr.infer_types(&FunctionTypeRegistry::empty()).unwrap();

            let compiled = compiler::compile(&expr, &vec![]).unwrap();
            let result = interpreter.run(compiled.byte_code).await.unwrap();

            assert_eq!(result.get_val().unwrap(), "1 foo bar".into_value_and_type());
        }

        #[test]
        async fn test_pattern_match_on_tuple_with_option_none() {
            let mut interpreter = Interpreter::default();

            let expr = r#"
           let x: tuple<u64, option<string>, string> = (1, none, "bar");

           match x {
              (x, none, z) => "${x} ${z}",
              (x, some(y), z) => "${x} ${y} ${z}"
           }
        "#;

            let expr = Expr::from_text(expr).unwrap();
            let compiled = compiler::compile(&expr, &vec![]).unwrap();
            let result = interpreter.run(compiled.byte_code).await.unwrap();

            assert_eq!(result.get_val().unwrap(), "1 bar".into_value_and_type());
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
                "foo 100 1 bar jak validate prod dev test".into_value_and_type()
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
                "dev 1 bar jak baz".into_value_and_type()
            );
        }

        #[test]
        async fn test_record_output_in_pattern_match() {
            let input_analysed_type = internal::get_analysed_type_record();
            let output_analysed_type = internal::get_analysed_type_result();

            let result_value = internal::get_value_and_type(&output_analysed_type, r#"ok(1)"#);

            let mut interpreter = internal::static_test_interpreter(&result_value);

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

            let expected = internal::get_value_and_type(
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
                internal::get_value_and_type(&output_analysed_type, r#"err("failed")"#);

            let mut interpreter = internal::static_test_interpreter(&result_value);

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

            let expected =
                internal::get_value_and_type(&tuple(vec![str(), str()]), r#"("failed", "bar")"#);

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
        use golem_wasm_rpc::IntoValueAndType;

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

            let result_value = internal::get_value_and_type(
                &result_type,
                r#"
          success({order-id: "foo"})
        "#,
            );

            let component_metadata =
                internal::get_shopping_cart_metadata_with_cart_resource_with_parameters();
            let compiled = compiler::compile(&expr, &component_metadata).unwrap();

            let mut rib_executor = internal::static_test_interpreter(&result_value);
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

            let result_value = internal::get_value_and_type(
                &result_type,
                r#"
            [{product-id: "foo", name: "bar", price: 100.0, quantity: 1}, {product-id: "bar", name: "baz", price: 200.0, quantity: 2}]
        "#,
            );

            let component_metadata =
                internal::get_shopping_cart_metadata_with_cart_resource_with_parameters();
            let compiled = compiler::compile(&expr, &component_metadata).unwrap();

            let mut rib_executor = internal::static_test_interpreter(&result_value);
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

            let component_metadata =
                internal::get_shopping_cart_metadata_with_cart_resource_with_parameters();

            let compiled = compiler::compile(&expr, &component_metadata).unwrap();

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

            let component_metadata =
                internal::get_shopping_cart_metadata_with_cart_resource_with_parameters();

            let compiled = compiler::compile(&expr, &component_metadata).unwrap();

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

            let component_metadata = internal::get_shopping_cart_metadata_with_cart_raw_resource();

            let compiled = compiler::compile(&expr, &component_metadata).unwrap();

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

            let result_value = internal::get_value_and_type(
                &result_type,
                r#"
            [{product-id: "foo", name: "bar", price: 100.0, quantity: 1}, {product-id: "bar", name: "baz", price: 200.0, quantity: 2}]
        "#,
            );

            let component_metadata = internal::get_shopping_cart_metadata_with_cart_raw_resource();
            let compiled = compiler::compile(&expr, &component_metadata).unwrap();

            let mut rib_executor = internal::static_test_interpreter(&result_value);
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

            let component_metadata = internal::get_shopping_cart_metadata_with_cart_raw_resource();

            let compiled = compiler::compile(&expr, &component_metadata).unwrap();

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

            let result_value = internal::get_value_and_type(
                &result_type,
                r#"
          success({order-id: "foo"})
        "#,
            );

            let component_metadata = internal::get_shopping_cart_metadata_with_cart_raw_resource();
            let compiled = compiler::compile(&expr, &component_metadata).unwrap();

            let mut rib_executor = internal::static_test_interpreter(&result_value);
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

            assert_eq!(result.get_val().unwrap(), "success".into_value_and_type());
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
        use golem_wasm_rpc::{Value, ValueAndType};
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

        pub(crate) fn get_value_and_type(
            analysed_type: &AnalysedType,
            wasm_wave_str: &str,
        ) -> ValueAndType {
            golem_wasm_rpc::parse_value_and_type(analysed_type, wasm_wave_str).unwrap()
        }

        pub(crate) fn static_test_interpreter(result_value: &ValueAndType) -> Interpreter {
            Interpreter {
                input: RibInput::default(),
                invoke: static_worker_invoke(result_value),
            }
        }

        fn static_worker_invoke(value: &ValueAndType) -> RibFunctionInvoke {
            let value = value.clone();

            Arc::new(move |_, _| {
                Box::pin({
                    let value = value.clone();

                    async move {
                        let value = value.clone();
                        Ok(ValueAndType::new(
                            Value::Tuple(vec![value.value]),
                            tuple(vec![value.typ]),
                        ))
                    }
                })
            })
        }
    }
}
