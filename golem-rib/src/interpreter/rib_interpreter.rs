// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use super::interpreter_stack_value::RibInterpreterStackValue;
use crate::interpreter::env::InterpreterEnv;
use crate::interpreter::instruction_cursor::RibByteCodeCursor;
use crate::interpreter::rib_runtime_error::{
    arithmetic_error, no_result, throw_error, RibRuntimeError,
};
use crate::interpreter::stack::InterpreterStack;
use crate::{
    internal_corrupted_state, DefaultWorkerNameGenerator, GenerateWorkerName, RibByteCode,
    RibComponentFunctionInvoke, RibIR, RibInput, RibResult,
};
use std::sync::Arc;

pub struct Interpreter {
    pub input: RibInput,
    pub invoke: Arc<dyn RibComponentFunctionInvoke + Sync + Send>,
    pub generate_worker_name: Arc<dyn GenerateWorkerName + Sync + Send>,
}

impl Default for Interpreter {
    fn default() -> Self {
        Interpreter {
            input: RibInput::default(),
            invoke: Arc::new(internal::NoopRibFunctionInvoke),
            generate_worker_name: Arc::new(DefaultWorkerNameGenerator),
        }
    }
}

pub type RibInterpreterResult<T> = Result<T, RibRuntimeError>;

impl Interpreter {
    pub fn new(
        input: RibInput,
        invoke: Arc<dyn RibComponentFunctionInvoke + Sync + Send>,
        generate_worker_name: Arc<dyn GenerateWorkerName + Sync + Send>,
    ) -> Self {
        Interpreter {
            input: input.clone(),
            invoke,
            generate_worker_name,
        }
    }

    // Interpreter that's not expected to call a side-effecting function call.
    // All it needs is environment with the required variables to evaluate the Rib script
    pub fn pure(
        input: RibInput,
        generate_worker_name: Arc<dyn GenerateWorkerName + Sync + Send>,
    ) -> Self {
        Interpreter {
            input,
            invoke: Arc::new(internal::NoopRibFunctionInvoke),
            generate_worker_name,
        }
    }

    pub fn override_rib_input(&mut self, rib_input: RibInput) {
        self.input = rib_input;
    }

    pub async fn run(&mut self, instructions0: RibByteCode) -> Result<RibResult, RibRuntimeError> {
        let mut byte_code_cursor = RibByteCodeCursor::from_rib_byte_code(instructions0);
        let mut stack = InterpreterStack::default();

        let mut interpreter_env = InterpreterEnv::from(&self.input, &self.invoke);

        while let Some(instruction) = byte_code_cursor.get_instruction() {
            match instruction {
                RibIR::GenerateWorkerName(instance_count) => {
                    internal::run_generate_worker_name(
                        instance_count,
                        self,
                        &mut stack,
                        &mut interpreter_env,
                    )?;
                }

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
                        |left, right| {
                            let result = left + right;
                            result.map_err(|err| arithmetic_error(err.as_str()))
                        },
                        &analysed_type,
                    )?;
                }
                RibIR::Minus(analysed_type) => {
                    internal::run_math_instruction(
                        &mut stack,
                        |left, right| {
                            let result = left - right;
                            result.map_err(|err| arithmetic_error(err.as_str()))
                        },
                        &analysed_type,
                    )?;
                }
                RibIR::Divide(analysed_type) => {
                    internal::run_math_instruction(
                        &mut stack,
                        |left, right| {
                            if right.is_zero() {
                                Err(arithmetic_error(
                                    format!("division by zero. left: {left}, right: {right}")
                                        .as_str(),
                                ))
                            } else {
                                (left / right).map_err(|err| arithmetic_error(err.as_str()))
                            }
                        },
                        &analysed_type,
                    )?;
                }
                RibIR::Multiply(analysed_type) => {
                    internal::run_math_instruction(
                        &mut stack,
                        |left, right| {
                            let result = left * right;
                            result.map_err(|err| arithmetic_error(err.as_str()))
                        },
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

                RibIR::SelectIndexV1 => {
                    internal::run_select_index_v1_instruction(&mut stack)?;
                }

                RibIR::CreateFunctionName(site, function_type) => {
                    internal::run_create_function_name_instruction(
                        site,
                        function_type,
                        &mut stack,
                    )?;
                }

                RibIR::InvokeFunction(
                    component_info,
                    instance_variable,
                    arg_size,
                    expected_result_type,
                ) => {
                    internal::run_invoke_function_instruction(
                        component_info,
                        &byte_code_cursor.position(),
                        arg_size,
                        instance_variable,
                        &mut stack,
                        &mut interpreter_env,
                        expected_result_type,
                    )
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
                    return Err(throw_error(message.as_str()));
                }

                RibIR::GetTag => {
                    internal::run_get_tag_instruction(&mut stack)?;
                }

                RibIR::Deconstruct => {
                    internal::run_deconstruct_instruction(&mut stack)?;
                }

                RibIR::Jump(instruction_id) => {
                    byte_code_cursor.move_to(&instruction_id).ok_or_else(|| {
                        internal_corrupted_state!(
                            "internal error. Failed to move to label {}",
                            instruction_id.index
                        )
                    })?;
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
                RibIR::ToIterator => {
                    internal::run_to_iterator(&mut stack)?;
                }
                RibIR::CreateSink(analysed_type) => {
                    internal::run_create_sink_instruction(&mut stack, analysed_type)?
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

                RibIR::Length => {
                    internal::run_length_instruction(&mut stack)?;
                }
            }
        }

        match byte_code_cursor.last() {
            Some(RibIR::AssignVar(_)) => Ok(RibResult::Unit),
            _ => {
                let stack_value = stack
                    .pop()
                    .unwrap_or_else(|| RibInterpreterStackValue::Unit);

                let rib_result = RibResult::from_rib_interpreter_stack_value(&stack_value)
                    .ok_or_else(no_result)?;
                Ok(rib_result)
            }
        }
    }
}

mod internal {
    use crate::interpreter::env::{EnvironmentKey, InterpreterEnv};
    use crate::interpreter::interpreter_stack_value::RibInterpreterStackValue;
    use crate::interpreter::literal::LiteralValue;
    use crate::interpreter::stack::InterpreterStack;
    use crate::{
        bail_corrupted_state, internal_corrupted_state, AnalysedTypeWithUnit, CoercedNumericValue,
        ComponentDependencyKey, EvaluatedFnArgs, EvaluatedFqFn, EvaluatedWorkerName,
        FunctionReferenceType, GetLiteralValue, InstanceVariable, InstructionId, Interpreter,
        ParsedFunctionName, ParsedFunctionReference, ParsedFunctionSite,
        RibComponentFunctionInvoke, RibFunctionInvokeResult, RibInterpreterResult, TypeHint,
        VariableId,
    };
    use golem_wasm_ast::analysis::AnalysedType;
    use golem_wasm_ast::analysis::TypeResult;
    use golem_wasm_rpc::{IntoValueAndType, Value, ValueAndType};

    use crate::interpreter::instruction_cursor::RibByteCodeCursor;
    use crate::interpreter::rib_runtime_error::{
        cast_error_custom, empty_stack, exhausted_iterator, field_not_found, function_invoke_fail,
        index_out_of_bound, infinite_computation, input_not_found, instruction_jump_error,
        insufficient_stack_items, invalid_type_with_stack_value, type_mismatch_with_type_hint,
        RibRuntimeError,
    };
    use crate::type_inference::GetTypeHint;
    use async_trait::async_trait;
    use golem_wasm_ast::analysis::analysed_type::{s16, s32, s64, s8, str, u16, u32, u64, u8};
    use std::ops::Deref;

    pub(crate) struct NoopRibFunctionInvoke;

    #[async_trait]
    impl RibComponentFunctionInvoke for NoopRibFunctionInvoke {
        async fn invoke(
            &self,
            _component_dependency_key: ComponentDependencyKey,
            _instruction_id: &InstructionId,
            _worker_name: Option<EvaluatedWorkerName>,
            _function_name: EvaluatedFqFn,
            _args: EvaluatedFnArgs,
            _return_type: Option<AnalysedType>,
        ) -> RibFunctionInvokeResult {
            Ok(None)
        }
    }

    pub(crate) fn run_is_empty_instruction(
        interpreter_stack: &mut InterpreterStack,
    ) -> RibInterpreterResult<()> {
        let rib_result = interpreter_stack.pop().ok_or_else(empty_stack)?;

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
                    internal_corrupted_state!(
                        "internal error: Expecting an iterator to check is empty"
                    )
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

        let bool = bool_opt.ok_or(internal_corrupted_state!("failed to execute is_empty"))?;
        interpreter_stack.push_val(bool.into_value_and_type());
        Ok(())
    }

    pub(crate) fn run_jump_if_false_instruction(
        instruction_id: InstructionId,
        instruction_stack: &mut RibByteCodeCursor,
        interpreter_stack: &mut InterpreterStack,
    ) -> RibInterpreterResult<()> {
        let predicate = interpreter_stack.try_pop_bool()?;

        // Jump if predicate is false
        if !predicate {
            instruction_stack
                .move_to(&instruction_id)
                .ok_or_else(|| instruction_jump_error(instruction_id))?;
        }

        Ok(())
    }

    macro_rules! match_range_to_value {
        (
        $from_val:expr,
        $to_val:expr,
        $variant:ident,
        $type_fn:expr,
        $inclusive:expr,
        $stack:expr
    ) => {
            match $to_val {
                Value::$variant(num2) => {
                    if $inclusive {
                        let range_iter = (*$from_val..=*num2)
                            .map(|i| ValueAndType::new(Value::$variant(i), $type_fn));
                        $stack.push(RibInterpreterStackValue::Iterator(Box::new(range_iter)));
                    } else {
                        let range_iter = (*$from_val..*num2)
                            .map(|i| ValueAndType::new(Value::$variant(i), $type_fn));
                        $stack.push(RibInterpreterStackValue::Iterator(Box::new(range_iter)));
                    }
                }

                _ => bail_corrupted_state!(concat!(
                    "expected a field named 'to' to be of type ",
                    stringify!($variant),
                    ", but it was not"
                )),
            }
        };
    }

    pub(crate) fn run_to_iterator(
        interpreter_stack: &mut InterpreterStack,
    ) -> RibInterpreterResult<()> {
        let popped_up = interpreter_stack.pop().ok_or_else(empty_stack)?;

        let value_and_type = popped_up.get_val().ok_or_else(empty_stack)?;

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
            (Value::Record(fields), AnalysedType::Record(_)) => {
                let from_value = fields.first().ok_or_else(|| {
                    internal_corrupted_state!(
                        "expected a field named 'from' to be present in the record"
                    )
                })?;

                let to_value = fields.get(1).ok_or_else(|| {
                    infinite_computation(
                        "an infinite range is being iterated. make sure range is finite to avoid infinite computation",
                    )
                })?;

                let inclusive_value = fields.get(2).ok_or_else(|| {
                    internal_corrupted_state!(
                        "expected a field named 'inclusive' to be present in the record"
                    )
                })?;

                let inclusive = match inclusive_value {
                    Value::Bool(b) => *b,
                    _ => {
                        bail_corrupted_state!(
                            "expected a field named 'inclusive' to be of type boolean, but it was not"
                        )
                    }
                };

                match from_value {
                    Value::S8(num1) => {
                        match_range_to_value!(num1, to_value, S8, s8(), inclusive, interpreter_stack);
                    }

                    Value::U8(num1) => {
                        match_range_to_value!(num1, to_value, U8, u8(), inclusive, interpreter_stack);
                    }

                    Value::S16(num1) => {
                        match_range_to_value!(num1, to_value, S16, s16(), inclusive, interpreter_stack);
                    }

                    Value::U16(num1) => {
                        match_range_to_value!(num1, to_value, U16, u16(), inclusive, interpreter_stack);
                    }

                    Value::S32(num1) => {
                        match_range_to_value!(num1, to_value, S32, s32(), inclusive, interpreter_stack);
                    }

                    Value::U32(num1) => {
                        match_range_to_value!(num1, to_value, U32, u32(), inclusive, interpreter_stack);
                    }

                    Value::S64(num1) => {
                        match_range_to_value!(num1, to_value, S64, s64(), inclusive, interpreter_stack);
                    }

                    Value::U64(num1) => {
                        match_range_to_value!(num1, to_value, U64, u64(), inclusive, interpreter_stack);
                    }

                    _ => bail_corrupted_state!(
                        "expected a field named 'from' to be of type S8, U8, S16, U16, S32, U32, S64, U64, but it was not"
                    ),
                }

                Ok(())
            }

            _ => Err(internal_corrupted_state!(
                "failed to convert to an iterator"
            )),
        }
    }

    pub(crate) fn run_create_sink_instruction(
        interpreter_stack: &mut InterpreterStack,
        analysed_type: AnalysedType,
    ) -> RibInterpreterResult<()> {
        let analysed_type = match analysed_type {
            AnalysedType::List(type_list) => *type_list.inner,
            _ => bail_corrupted_state!("expecting a list type to create sink"),
        };
        interpreter_stack.create_sink(analysed_type);
        Ok(())
    }

    pub(crate) fn run_advance_iterator_instruction(
        interpreter_stack: &mut InterpreterStack,
    ) -> RibInterpreterResult<()> {
        let mut stack_value = interpreter_stack.pop().ok_or_else(empty_stack)?;

        match &mut stack_value {
            RibInterpreterStackValue::Sink(_, _) => {
                let mut existing_iterator = interpreter_stack
                    .pop()
                    .ok_or(internal_corrupted_state!("failed to get an iterator"))?;

                match &mut existing_iterator {
                    RibInterpreterStackValue::Iterator(iter) => {
                        if let Some(value_and_type) = iter.next() {
                            interpreter_stack.push(existing_iterator); // push the iterator back
                            interpreter_stack.push(stack_value); // push the sink back
                            interpreter_stack.push(RibInterpreterStackValue::Val(value_and_type));
                            Ok(())
                        } else {
                            Err(exhausted_iterator())
                        }
                    }

                    _ => Err(internal_corrupted_state!(
                        "sink cannot exist without a corresponding iterator"
                    )),
                }
            }

            RibInterpreterStackValue::Iterator(iter) => {
                if let Some(value_and_type) = iter.next() {
                    interpreter_stack.push(stack_value);
                    interpreter_stack.push(RibInterpreterStackValue::Val(value_and_type));
                    Ok(())
                } else {
                    Err(exhausted_iterator())
                }
            }
            _ => Err(exhausted_iterator()),
        }
    }

    pub(crate) fn run_push_to_sink_instruction(
        interpreter_stack: &mut InterpreterStack,
    ) -> RibInterpreterResult<()> {
        let last_value = interpreter_stack.pop_val();
        match last_value {
            Some(val) => {
                interpreter_stack.push_to_sink(val)?;

                Ok(())
            }
            None => Ok(()),
        }
    }

    pub(crate) fn run_sink_to_list_instruction(
        interpreter_stack: &mut InterpreterStack,
    ) -> RibInterpreterResult<()> {
        let (result, analysed_type) =
            interpreter_stack
                .pop_sink()
                .ok_or(internal_corrupted_state!(
                    "failed to retrieve items from sink"
                ))?;

        interpreter_stack.push_list(
            result.into_iter().map(|vnt| vnt.value).collect(),
            &analysed_type,
        );

        Ok(())
    }

    pub(crate) fn run_length_instruction(
        interpreter_stack: &mut InterpreterStack,
    ) -> RibInterpreterResult<()> {
        let rib_result = interpreter_stack.pop().ok_or_else(empty_stack)?;

        let length = match rib_result {
            RibInterpreterStackValue::Val(ValueAndType {
                value: Value::List(items),
                ..
            }) => items.len(),
            RibInterpreterStackValue::Iterator(iter) => iter.count(),
            _ => bail_corrupted_state!("failed to get the length of the value"),
        };

        interpreter_stack.push_val(ValueAndType::new(Value::U64(length as u64), u64()));
        Ok(())
    }

    pub(crate) fn run_assign_var_instruction(
        variable_id: VariableId,
        interpreter_stack: &mut InterpreterStack,
        interpreter_env: &mut InterpreterEnv,
    ) -> RibInterpreterResult<()> {
        let value = interpreter_stack.pop().ok_or_else(empty_stack)?;
        let env_key = EnvironmentKey::from(variable_id);

        interpreter_env.insert(env_key, value);
        Ok(())
    }

    pub(crate) fn run_load_var_instruction(
        variable_id: VariableId,
        interpreter_stack: &mut InterpreterStack,
        interpreter_env: &mut InterpreterEnv,
    ) -> RibInterpreterResult<()> {
        let env_key = EnvironmentKey::from(variable_id.clone());
        let value = interpreter_env
            .lookup(&env_key)
            .ok_or_else(|| input_not_found(variable_id.name().as_str()))?;

        match value {
            RibInterpreterStackValue::Unit => {
                interpreter_stack.push(RibInterpreterStackValue::Unit);
            }
            RibInterpreterStackValue::Val(val) => interpreter_stack.push_val(val.clone()),
            RibInterpreterStackValue::Iterator(_) => {
                bail_corrupted_state!("internal error: unable to assign an iterator to a variable")
            }
            RibInterpreterStackValue::Sink(_, _) => {
                bail_corrupted_state!("internal error: unable to assign a sink to a variable")
            }
        }

        Ok(())
    }

    pub(crate) fn run_generate_worker_name(
        variable_id: Option<VariableId>,
        interpreter: &mut Interpreter,
        interpreter_stack: &mut InterpreterStack,
        interpreter_env: &mut InterpreterEnv,
    ) -> RibInterpreterResult<()> {
        match variable_id {
            None => {
                let worker_name = interpreter.generate_worker_name.generate_worker_name();

                interpreter_stack
                    .push_val(ValueAndType::new(Value::String(worker_name.clone()), str()));
            }

            Some(variable_id) => {
                let instance_variable = variable_id.as_instance_variable();

                let env_key = EnvironmentKey::from(instance_variable);

                let worker_id = interpreter_env.lookup(&env_key);

                match worker_id {
                    Some(worker_id) => {
                        let value_and_type = worker_id.get_val().ok_or_else(|| {
                            internal_corrupted_state!(
                        "expected a worker name to be present in the environment, but it was not found"
                    )
                        })?;

                        interpreter_stack.push_val(value_and_type);
                    }

                    None => {
                        let worker_name = interpreter.generate_worker_name.generate_worker_name();

                        interpreter_stack
                            .push_val(ValueAndType::new(Value::String(worker_name.clone()), str()));
                    }
                }
            }
        }

        Ok(())
    }

    pub(crate) fn run_create_record_instruction(
        analysed_type: AnalysedType,
        interpreter_stack: &mut InterpreterStack,
    ) -> RibInterpreterResult<()> {
        let name_type_pair = match analysed_type {
            AnalysedType::Record(type_record) => type_record.fields,
            _ => {
                bail_corrupted_state!(
                    "expected a record type to create a record, but obtained {}",
                    analysed_type.get_type_hint()
                )
            }
        };

        interpreter_stack.create_record(name_type_pair);
        Ok(())
    }

    pub(crate) fn run_update_record_instruction(
        field_name: String,
        interpreter_stack: &mut InterpreterStack,
    ) -> RibInterpreterResult<()> {
        let (current_record_fields, record_type) = interpreter_stack.try_pop_record()?;

        let idx = record_type
            .fields
            .iter()
            .position(|pair| pair.name == field_name)
            .ok_or_else(|| {
                internal_corrupted_state!(
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
    ) -> RibInterpreterResult<()> {
        match analysed_type {
            AnalysedType::List(inner_type) => {
                let items = interpreter_stack.try_pop_n_val(list_size)?;

                interpreter_stack.push_list(
                    items.into_iter().map(|vnt| vnt.value).collect(),
                    inner_type.inner.deref(),
                );

                Ok(())
            }

            _ => Err(internal_corrupted_state!(
                "failed to create list due to mismatch in types. expected: list, actual: {}",
                analysed_type.get_type_hint()
            )),
        }
    }

    pub(crate) fn run_push_tuple_instruction(
        list_size: usize,
        analysed_type: AnalysedType,
        interpreter_stack: &mut InterpreterStack,
    ) -> RibInterpreterResult<()> {
        match analysed_type {
            AnalysedType::Tuple(_inner_type) => {
                let items = interpreter_stack.try_pop_n_val(list_size)?;
                interpreter_stack.push_tuple(items);
                Ok(())
            }

            _ => Err(internal_corrupted_state!(
                "failed to create tuple due to mismatch in types. expected: tuple, actual: {}",
                analysed_type.get_type_hint()
            )),
        }
    }

    pub(crate) fn run_negate_instruction(
        interpreter_stack: &mut InterpreterStack,
    ) -> RibInterpreterResult<()> {
        let bool = interpreter_stack.try_pop_bool()?;
        let negated = !bool;

        interpreter_stack.push_val(negated.into_value_and_type());
        Ok(())
    }

    pub(crate) fn run_and_instruction(
        interpreter_stack: &mut InterpreterStack,
    ) -> RibInterpreterResult<()> {
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
    ) -> RibInterpreterResult<()> {
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
        compare_fn: fn(
            CoercedNumericValue,
            CoercedNumericValue,
        ) -> Result<CoercedNumericValue, RibRuntimeError>,
        target_numerical_type: &AnalysedType,
    ) -> RibInterpreterResult<()> {
        let left = interpreter_stack.try_pop()?;
        let right = interpreter_stack.try_pop()?;

        let result = left.evaluate_math_op(&right, compare_fn)?;
        let numerical_type = result
            .cast_to(target_numerical_type)
            .ok_or_else(|| cast_error_custom(result, target_numerical_type.get_type_hint()))?;

        interpreter_stack.push_val(numerical_type);

        Ok(())
    }

    pub(crate) fn run_compare_instruction(
        interpreter_stack: &mut InterpreterStack,
        compare_fn: fn(LiteralValue, LiteralValue) -> bool,
    ) -> RibInterpreterResult<()> {
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
    ) -> RibInterpreterResult<()> {
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
                    .ok_or_else(|| field_not_found(field_name.as_str()))?;

                let value = field.0;
                interpreter_stack.push_val(ValueAndType::new(value, field.1.typ));
                Ok(())
            }
            _ => Err(field_not_found(field_name.as_str())),
        }
    }

    pub(crate) fn run_select_index_v1_instruction(
        interpreter_stack: &mut InterpreterStack,
    ) -> RibInterpreterResult<()> {
        let stack_list_value = interpreter_stack.pop().ok_or_else(empty_stack)?;

        let index_value = interpreter_stack.pop().ok_or(empty_stack())?;

        match stack_list_value {
            RibInterpreterStackValue::Val(ValueAndType {
                value: Value::List(items),
                typ: AnalysedType::List(typ),
            }) => match index_value.get_literal().and_then(|v| v.get_number()) {
                Some(CoercedNumericValue::PosInt(index)) => {
                    let value = items
                        .get(index as usize)
                        .ok_or_else(|| index_out_of_bound(index as usize, items.len()))?
                        .clone();

                    interpreter_stack.push_val(ValueAndType::new(value, (*typ.inner).clone()));
                    Ok(())
                }
                Some(CoercedNumericValue::NegInt(index)) => {
                    if index >= 0 {
                        let value = items
                            .get(index as usize)
                            .ok_or_else(|| index_out_of_bound(index as usize, items.len()))?
                            .clone();

                        interpreter_stack.push_val(ValueAndType::new(value, (*typ.inner).clone()));
                    } else {
                        return Err(index_out_of_bound(index as usize, items.len()));
                    }
                    Ok(())
                }

                _ => Err(internal_corrupted_state!("failed range selection")),
            },
            RibInterpreterStackValue::Val(ValueAndType {
                value: Value::Tuple(items),
                typ: AnalysedType::Tuple(typ),
            }) => match index_value.get_literal().and_then(|v| v.get_number()) {
                Some(CoercedNumericValue::PosInt(index)) => {
                    let value = items
                        .get(index as usize)
                        .ok_or_else(|| index_out_of_bound(index as usize, items.len()))?
                        .clone();

                    let item_type = typ
                        .items
                        .get(index as usize)
                        .ok_or_else(|| {
                            internal_corrupted_state!(
                                "type not found in the tuple at index {}",
                                index
                            )
                        })?
                        .clone();

                    interpreter_stack.push_val(ValueAndType::new(value, item_type));
                    Ok(())
                }
                _ => Err(invalid_type_with_stack_value(
                    vec![TypeHint::Number],
                    index_value,
                )),
            },
            result => Err(invalid_type_with_stack_value(
                vec![TypeHint::List(None), TypeHint::Tuple(None)],
                result,
            )),
        }
    }

    pub(crate) fn run_select_index_instruction(
        interpreter_stack: &mut InterpreterStack,
        index: usize,
    ) -> RibInterpreterResult<()> {
        let stack_value = interpreter_stack.pop().ok_or_else(empty_stack)?;

        match stack_value {
            RibInterpreterStackValue::Val(ValueAndType {
                value: Value::List(items),
                typ: AnalysedType::List(typ),
            }) => {
                let value = items
                    .get(index)
                    .ok_or_else(|| index_out_of_bound(index, items.len()))?
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
                    .ok_or_else(|| index_out_of_bound(index, items.len()))?
                    .clone();

                let item_type = typ
                    .items
                    .get(index)
                    .ok_or_else(|| index_out_of_bound(index, items.len()))?
                    .clone();

                interpreter_stack.push_val(ValueAndType::new(value, item_type));
                Ok(())
            }
            result => Err(invalid_type_with_stack_value(
                vec![TypeHint::List(None), TypeHint::Tuple(None)],
                result,
            )),
        }
    }

    pub(crate) fn run_push_enum_instruction(
        interpreter_stack: &mut InterpreterStack,
        enum_name: String,
        analysed_type: AnalysedType,
    ) -> RibInterpreterResult<()> {
        match analysed_type {
            AnalysedType::Enum(typed_enum) => {
                interpreter_stack.push_enum(enum_name, typed_enum.cases)?;
                Ok(())
            }
            _ => Err(type_mismatch_with_type_hint(
                vec![TypeHint::Enum(None)],
                analysed_type.get_type_hint(),
            )),
        }
    }

    pub(crate) async fn run_variant_construction_instruction(
        variant_name: String,
        analysed_type: AnalysedType,
        interpreter_stack: &mut InterpreterStack,
    ) -> RibInterpreterResult<()> {
        match analysed_type {
            AnalysedType::Variant(variants) => {
                let variant = variants
                    .cases
                    .iter()
                    .find(|name| name.name == variant_name)
                    .ok_or_else(|| {
                        internal_corrupted_state!("variant {} not found", variant_name)
                    })?;

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

            _ => Err(type_mismatch_with_type_hint(
                vec![TypeHint::Variant(None)],
                analysed_type.get_type_hint(),
            )),
        }
    }

    pub(crate) fn run_create_function_name_instruction(
        site: ParsedFunctionSite,
        function_type: FunctionReferenceType,
        interpreter_stack: &mut InterpreterStack,
    ) -> RibInterpreterResult<()> {
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
                    .ok_or_else(|| insufficient_stack_items(arg_size))?;

                let parameter_values = last_n_elements
                    .iter()
                    .map(|interpreter_result| {
                        interpreter_result.get_val().ok_or_else(|| {
                            internal_corrupted_state!("failed to construct resource")
                        })
                    })
                    .collect::<RibInterpreterResult<Vec<ValueAndType>>>()?;

                let parsed_function_name = ParsedFunctionName {
                    site,
                    function: ParsedFunctionReference::IndexedResourceConstructor {
                        resource,
                        resource_params: parameter_values
                            .iter()
                            .map(|x| x.to_string())
                            .collect::<Vec<_>>(),
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
                    .ok_or_else(|| insufficient_stack_items(arg_size))?;

                let param_values = last_n_elements
                    .iter()
                    .map(|interpreter_result| {
                        interpreter_result.get_val().ok_or_else(|| {
                            internal_corrupted_state!(
                                "internal error: failed to call indexed resource method {}",
                                method
                            )
                        })
                    })
                    .collect::<RibInterpreterResult<Vec<ValueAndType>>>()?;

                let parsed_function_name = ParsedFunctionName {
                    site,
                    function: ParsedFunctionReference::IndexedResourceMethod {
                        resource,
                        resource_params: param_values
                            .iter()
                            .map(|x| x.to_string())
                            .collect::<Vec<String>>(),
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
                let last_n_elements = interpreter_stack
                    .pop_n(arg_size)
                    .ok_or_else(|| insufficient_stack_items(arg_size))?;

                let param_values = last_n_elements
                    .iter()
                    .map(|interpreter_result| {
                        interpreter_result.get_val().ok_or_else(|| {
                            internal_corrupted_state!(
                                "failed to call static resource method {}",
                                method
                            )
                        })
                    })
                    .collect::<RibInterpreterResult<Vec<ValueAndType>>>()?;

                let parsed_function_name = ParsedFunctionName {
                    site,
                    function: ParsedFunctionReference::IndexedResourceStaticMethod {
                        resource,
                        resource_params: param_values
                            .iter()
                            .map(|x| x.to_string())
                            .collect::<Vec<_>>(),
                        method,
                    },
                };

                interpreter_stack.push_val(parsed_function_name.to_string().into_value_and_type());
            }
            FunctionReferenceType::IndexedResourceDrop { resource, arg_size } => {
                let last_n_elements = interpreter_stack
                    .pop_n(arg_size)
                    .ok_or_else(|| insufficient_stack_items(arg_size))?;

                let param_values = last_n_elements
                    .iter()
                    .map(|interpreter_result| {
                        interpreter_result.get_val().ok_or_else(|| {
                            internal_corrupted_state!("failed to call indexed resource drop")
                        })
                    })
                    .collect::<RibInterpreterResult<Vec<ValueAndType>>>()?;

                let parsed_function_name = ParsedFunctionName {
                    site,
                    function: ParsedFunctionReference::IndexedResourceDrop {
                        resource,
                        resource_params: param_values
                            .iter()
                            .map(|x| x.to_string())
                            .collect::<Vec<_>>(),
                    },
                };

                interpreter_stack.push_val(parsed_function_name.to_string().into_value_and_type());
            }
        }

        Ok(())
    }

    pub(crate) async fn run_invoke_function_instruction(
        component_info: ComponentDependencyKey,
        instruction_id: &InstructionId,
        arg_size: usize,
        instance_variable_type: InstanceVariable,
        interpreter_stack: &mut InterpreterStack,
        interpreter_env: &mut InterpreterEnv,
        expected_result_type: AnalysedTypeWithUnit,
    ) -> RibInterpreterResult<()> {
        let function_name = interpreter_stack
            .pop_str()
            .ok_or_else(|| internal_corrupted_state!("failed to get a function name"))?;

        let function_name_cloned = function_name.clone();

        let last_n_elements = interpreter_stack
            .pop_n(arg_size)
            .ok_or_else(|| insufficient_stack_items(arg_size))?;

        let expected_result_type = match expected_result_type {
            AnalysedTypeWithUnit::Type(analysed_type) => Some(analysed_type),
            AnalysedTypeWithUnit::Unit => None,
        };

        let parameter_values = last_n_elements
            .iter()
            .map(|interpreter_result| {
                interpreter_result.get_val().ok_or_else(|| {
                    internal_corrupted_state!("failed to call function {}", function_name)
                })
            })
            .collect::<RibInterpreterResult<Vec<ValueAndType>>>()?;

        match instance_variable_type {
            InstanceVariable::WitWorker(variable_id) => {
                let worker_id = interpreter_env
                    .lookup(&EnvironmentKey::from(variable_id.clone()))
                    .map(|x| {
                        x.get_val().ok_or_else(|| {
                            internal_corrupted_state!(
                                "failed to get a worker variable id for function {}",
                                function_name
                            )
                        })
                    })
                    .transpose()?
                    .ok_or_else(|| {
                        internal_corrupted_state!(
                            "failed to find a worker with id {}",
                            variable_id.name()
                        )
                    })?;

                let worker_id_string =
                    worker_id
                        .get_literal()
                        .map(|v| v.as_string())
                        .ok_or_else(|| {
                            internal_corrupted_state!("failed to get a worker name for variable")
                        })?;

                let result = interpreter_env
                    .invoke_worker_function_async(
                        component_info,
                        instruction_id,
                        Some(worker_id_string),
                        function_name_cloned,
                        parameter_values,
                        expected_result_type.clone(),
                    )
                    .await
                    .map_err(|err| function_invoke_fail(function_name.as_str(), err))?;

                match result {
                    None => {
                        interpreter_stack.push(RibInterpreterStackValue::Unit);
                    }
                    Some(result) => {
                        interpreter_stack.push(RibInterpreterStackValue::Val(result));
                    }
                }
            }

            InstanceVariable::WitResource(variable_id)
                if variable_id == VariableId::global("___STATIC_WIT_RESOURCE".to_string()) =>
            {
                let result = interpreter_env
                    .invoke_worker_function_async(
                        component_info,
                        instruction_id,
                        None,
                        function_name_cloned,
                        parameter_values,
                        expected_result_type.clone(),
                    )
                    .await
                    .map_err(|err| function_invoke_fail(function_name.as_str(), err))?;

                match result {
                    None => {
                        interpreter_stack.push(RibInterpreterStackValue::Unit);
                    }
                    Some(result) => {
                        interpreter_stack.push(RibInterpreterStackValue::Val(result));
                    }
                }
            }

            InstanceVariable::WitResource(variable_id) => {
                let mut final_args = vec![];

                let handle = interpreter_env
                    .lookup(&EnvironmentKey::from(variable_id.clone()))
                    .map(|x| {
                        x.get_val().ok_or_else(|| {
                            internal_corrupted_state!(
                                "failed to get a resource with id {}",
                                variable_id.name()
                            )
                        })
                    })
                    .transpose()?
                    .ok_or_else(|| {
                        internal_corrupted_state!(
                            "failed to find a resource with id {}",
                            variable_id.name()
                        )
                    })?;

                match &handle.value {
                    Value::Handle { uri, .. } => {
                        let worker_name = uri.rsplit_once('/').map(|(_, last)| last).unwrap_or(uri);

                        final_args.push(handle.clone());
                        final_args.extend(parameter_values);

                        let result = interpreter_env
                            .invoke_worker_function_async(
                                component_info,
                                instruction_id,
                                Some(worker_name.to_string()),
                                function_name_cloned.clone(),
                                final_args,
                                expected_result_type.clone(),
                            )
                            .await
                            .map_err(|err| function_invoke_fail(function_name.as_str(), err))?;

                        match result {
                            None => {
                                interpreter_stack.push(RibInterpreterStackValue::Unit);
                            }
                            Some(result) => {
                                interpreter_stack.push(RibInterpreterStackValue::Val(result));
                            }
                        }
                    }

                    _ => {
                        return Err(function_invoke_fail(
                            function_name.as_str(),
                            "expected the result of a resource construction to be of type `handle`"
                                .into(),
                        ))
                    }
                };
            }
        };

        Ok(())
    }

    pub(crate) fn run_deconstruct_instruction(
        interpreter_stack: &mut InterpreterStack,
    ) -> RibInterpreterResult<()> {
        let value = interpreter_stack
            .pop()
            .ok_or_else(|| internal_corrupted_state!("no value to unwrap"))?;

        let unwrapped_value = value
            .unwrap()
            .ok_or_else(|| internal_corrupted_state!("failed to unwrap the value {}", value))?;

        interpreter_stack.push_val(unwrapped_value);
        Ok(())
    }

    pub(crate) fn run_get_tag_instruction(
        interpreter_stack: &mut InterpreterStack,
    ) -> RibInterpreterResult<()> {
        let value = interpreter_stack
            .pop_val()
            .ok_or_else(|| internal_corrupted_state!("failed to get a tag value"))?;

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
    ) -> RibInterpreterResult<()> {
        let value = interpreter_stack.try_pop_val()?;

        match analysed_type {
            AnalysedType::Option(analysed_type) => {
                interpreter_stack.push_some(value.value, analysed_type.inner.deref());
                Ok(())
            }
            _ => Err(type_mismatch_with_type_hint(
                vec![TypeHint::Option(None)],
                analysed_type.get_type_hint(),
            )),
        }
    }

    pub(crate) fn run_create_none_instruction(
        interpreter_stack: &mut InterpreterStack,
        analysed_type: Option<AnalysedType>,
    ) -> RibInterpreterResult<()> {
        match analysed_type {
            Some(AnalysedType::Option(_)) | None => {
                interpreter_stack.push_none(analysed_type);
                Ok(())
            }
            _ => Err(type_mismatch_with_type_hint(
                vec![TypeHint::Option(None)],
                analysed_type
                    .as_ref()
                    .map(|t| t.get_type_hint())
                    .unwrap_or_else(|| TypeHint::Unknown),
            )),
        }
    }

    pub(crate) fn run_create_ok_instruction(
        interpreter_stack: &mut InterpreterStack,
        analysed_type: AnalysedType,
    ) -> RibInterpreterResult<()> {
        let value = interpreter_stack.try_pop_val()?;

        match analysed_type {
            AnalysedType::Result(TypeResult { ok, err, .. }) => {
                interpreter_stack.push_ok(value.value, ok.as_deref(), err.as_deref());
                Ok(())
            }
            _ => Err(type_mismatch_with_type_hint(
                vec![TypeHint::Result {
                    ok: None,
                    err: None,
                }],
                analysed_type.get_type_hint(),
            )),
        }
    }

    pub(crate) fn run_create_err_instruction(
        interpreter_stack: &mut InterpreterStack,
        analysed_type: AnalysedType,
    ) -> RibInterpreterResult<()> {
        let value = interpreter_stack.try_pop_val()?;

        match analysed_type {
            AnalysedType::Result(TypeResult { ok, err, .. }) => {
                interpreter_stack.push_err(value.value, ok.as_deref(), err.as_deref());
                Ok(())
            }
            _ => Err(type_mismatch_with_type_hint(
                vec![TypeHint::Result {
                    ok: None,
                    err: None,
                }],
                analysed_type.get_type_hint(),
            )),
        }
    }

    pub(crate) fn run_concat_instruction(
        interpreter_stack: &mut InterpreterStack,
        arg_size: usize,
    ) -> RibInterpreterResult<()> {
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
        get_analysed_type_variant, get_value_and_type, strip_spaces, RibTestDeps,
    };
    use crate::{
        Expr, GlobalVariableTypeSpec, InferredType, InstructionId, Path, RibCompiler,
        RibCompilerConfig, VariableId,
    };
    use golem_wasm_ast::analysis::analysed_type::{
        bool, f32, field, list, r#enum, record, result, s32, str, tuple, u32, u64, u8,
    };
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
               let x = 1;
               let y = x + 2;
               y
            "#;

        let expr = Expr::from_text(rib_expr).unwrap();

        let mut interpreter = Interpreter::default();

        let compiler = RibCompiler::default();

        let compiled = compiler.compile(expr).unwrap();

        let result = interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(result.get_val().unwrap(), 3i32.into_value_and_type());
    }

    #[test]
    async fn test_interpreter_variable_scope_1() {
        let rib_expr = r#"
               let x = 1;
               let z = {foo : x};
               let x = x + 2u64;
               { bar: x, baz: z }
            "#;

        let expr = Expr::from_text(rib_expr).unwrap();

        let mut interpreter = Interpreter::default();

        let compiler = RibCompiler::default();
        let compiled = compiler.compile(expr).unwrap();

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
               let x = 1;
               let x = x;

               let result1 = match some(x + 1) {
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

        let mut interpreter = Interpreter::default();

        let compiler = RibCompiler::default();

        let compiled = compiler.compile(expr).unwrap();

        let result = interpreter.run(compiled.byte_code).await.unwrap();

        let analysed_type = record(vec![field("result1", u64()), field("result2", u64())]);

        let expected = get_value_and_type(&analysed_type, r#"{ result1: 2, result2: 1 }"#);

        assert_eq!(result.get_val().unwrap(), expected);
    }

    #[test]
    async fn test_interpreter_variable_scope_3() {
        let rib_expr = r#"
               let x = 1;
               let x = x;

               let result1 = match some(x + 1) {
                  some(x) => match some(x + 1) {
                     some(x) => x,
                     none => x
                  },
                  none => x
               };

               let z: option<u64> = none;

               let result2 = match z {
                  some(x) => x,
                  none => match some(x + 1) {
                     some(x) => x,
                     none => x
                  }
               };

               { result1: result1, result2: result2 }
            "#;

        let expr = Expr::from_text(rib_expr).unwrap();

        let mut interpreter = Interpreter::default();

        let compiler = RibCompiler::default();

        let compiled = compiler.compile(expr).unwrap();

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
            GlobalVariableTypeSpec::new(
                "request",
                Path::from_elems(vec!["path"]),
                InferredType::string(),
            ),
            GlobalVariableTypeSpec::new(
                "request",
                Path::from_elems(vec!["headers"]),
                InferredType::string(),
            ),
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

        let mut interpreter =
            test_utils::interpreter_with_noop_function_invoke(Some(RibInput::new(rib_input)));

        let expr = Expr::from_text(rib_expr).unwrap();

        let compiler = RibCompiler::new(RibCompilerConfig::new(vec![], type_spec));
        let compiled = compiler.compile(expr).unwrap();

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
            GlobalVariableTypeSpec::new(
                "request",
                Path::from_elems(vec!["path"]),
                InferredType::string(),
            ),
            GlobalVariableTypeSpec::new(
                "request",
                Path::from_elems(vec!["headers"]),
                InferredType::string(),
            ),
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

        let mut interpreter =
            test_utils::interpreter_with_noop_function_invoke(Some(RibInput::new(rib_input)));

        let expr = Expr::from_text(rib_expr).unwrap();

        let compiler = RibCompiler::new(RibCompilerConfig::new(vec![], type_spec));

        let compiled = compiler.compile(expr).unwrap();

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
        let mut interpreter = Interpreter::default();

        let rib_expr = r#"
            let x = "foo";
            let y = "bar";
            let z = {foo: "baz"};
            let n: u32 = 42;
            let result = "${x}-${y}-${z}-${n}";
            result
        "#;

        let expr = Expr::from_text(rib_expr).unwrap();

        let compiler = RibCompiler::default();

        let compiled = compiler.compile(expr).unwrap();

        let result = interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(
            result.get_val().unwrap().value,
            Value::String("foo-bar-{foo: \"baz\"}-42".to_string())
        );
    }

    #[test]
    async fn test_interpreter_with_variant_and_enum() {
        let test_deps = RibTestDeps::test_deps_with_global_functions();

        let compiler = RibCompiler::new(RibCompilerConfig::new(
            test_deps.component_dependencies.clone(),
            vec![],
        ));

        let mut interpreter = test_deps.interpreter;

        // This has intentionally got conflicting variable names
        // variable `x` is same as the enum name `x`
        // similarly, variably `validate` is same as the variant name validate
        let expr = r#"
          let x = x;
          let y = x;
          let a = instance();
          let result1 = a.add-enum(x, y);
          let validate = validate;
          let validate2 = validate;
          let result2 = a.add-variant(validate, validate2);
          {res1: result1, res2: result2}
        "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiled = compiler.compile(expr);

        let result = interpreter.run(compiled.unwrap().byte_code).await.unwrap();
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
        let test_deps = RibTestDeps::test_deps_with_global_functions();

        let compiler = RibCompiler::new(RibCompilerConfig::new(
            test_deps.component_dependencies.clone(),
            vec![],
        ));

        let mut interpreter = test_deps.interpreter;

        // This has intentionally conflicting variable names
        // variable `x` is same as the enum name `x`
        // similarly, variably `validate` is same as the variant name `validate`
        // and `process-user` is same as the variant name `process-user`
        let expr = r#"
          let x = 1;
          let y = 2;
          let a = instance();
          let result1 = a.add-u32(x, y);
          let process-user = 3;
          let validate = 4;
          let result2 = a.add-u64(process-user, validate);
          {res1: result1, res2: result2}
        "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiled = compiler.compile(expr).unwrap();
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
        let compiler = RibCompiler::default();
        let compiled = compiler.compile(expr).unwrap();
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

        let compiler = RibCompiler::default();
        let compiled = compiler.compile(expr).unwrap();

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

        let compiler = RibCompiler::default();

        let compiled = compiler.compile(expr).unwrap();

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

        let compiler = RibCompiler::default();

        let compiled = compiler.compile(expr).unwrap();

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
        let component_metadata = test_utils::configurable_metadata("foo", vec![u32()], Some(u64()));

        let mut interpreter = test_utils::interpreter_with_static_function_response(
            &ValueAndType::new(Value::U64(2), u64()),
            None,
        );

        // 1 is automatically inferred to be u32
        let rib = r#"
          let worker = instance("my-worker");
          worker.foo(1)
        "#;

        let expr = Expr::from_text(rib).unwrap();

        let compiler_config = RibCompilerConfig::new(component_metadata, vec![]);
        let compiler = RibCompiler::new(compiler_config);

        let compiled = compiler.compile(expr).unwrap();
        let result = interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(
            result.get_val().unwrap(),
            ValueAndType::new(Value::U64(2), u64())
        );
    }

    #[test]
    async fn test_interpreter_with_numbers_2() {
        let component_metadata = test_utils::configurable_metadata("foo", vec![u32()], Some(u64()));

        let mut interpreter = test_utils::interpreter_with_static_function_response(
            &ValueAndType::new(Value::U64(2), u64()),
            None,
        );

        // 1 and 2 are automatically inferred to be u32
        // since the type of z is inferred to be u32 as that being passed to a function
        // that expects u32
        let rib = r#"
          let worker = instance("my-worker");
          let z = 1 + 2;
          worker.foo(z)
        "#;

        let expr = Expr::from_text(rib).unwrap();

        let compiler_config = RibCompilerConfig::new(component_metadata, vec![]);
        let compiler = RibCompiler::new(compiler_config);

        let compiled = compiler.compile(expr).unwrap();
        let result = interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(
            result.get_val().unwrap(),
            ValueAndType::new(Value::U64(2), u64())
        );
    }

    #[test]
    async fn test_interpreter_with_numbers_3() {
        let component_metadata = test_utils::configurable_metadata("foo", vec![u32()], Some(u64()));

        // This will cause a type inference error
        // because the operands of the + operator are not of the same type
        let rib = r#"
          let worker = instance("my-worker");
          let z = 1: u8 + 2;
          worker.foo(z)
        "#;

        let expr = Expr::from_text(rib).unwrap();
        let compiler = RibCompiler::new(RibCompilerConfig::new(component_metadata, vec![]));
        let compile_result = compiler.compile(expr);
        assert!(compile_result.is_err());
    }

    #[test]
    async fn test_interpreter_with_numbers_4() {
        let component_metadata = test_utils::configurable_metadata("foo", vec![u32()], Some(u64()));

        // This will cause a type inference error
        // because the operands of the + operator are supposed to be u32
        // since z is u32
        let rib = r#"
          let worker = instance("my-worker");
          let z = 1: u8 + 2: u8;
          worker.foo(z)
        "#;

        let expr = Expr::from_text(rib).unwrap();
        let compiler = RibCompiler::new(RibCompilerConfig::new(component_metadata, vec![]));
        let compile_result = compiler.compile(expr);
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

        let compiler = RibCompiler::default();
        let compiled = compiler.compile(expr).unwrap();

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

        let compiler = RibCompiler::default();
        let compiled = compiler.compile(expr).unwrap();

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

        let expr = Expr::from_text(expr).unwrap();
        let compiler = RibCompiler::default();
        let compiled = compiler.compile(expr).unwrap();
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

        let expr = Expr::from_text(expr).unwrap();
        let compiler = RibCompiler::default();
        let compiled = compiler.compile(expr).unwrap();
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

        let expr = Expr::from_text(expr).unwrap();
        let compiler = RibCompiler::default();
        let compiled = compiler.compile(expr).unwrap();
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
        let compiler = RibCompiler::default();
        let compiled = compiler.compile(expr).unwrap();
        let result = interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(result.get_val().unwrap(), "1 bar".into_value_and_type());
    }

    #[test]
    async fn test_interpreter_pattern_match_dynamic_branch_1() {
        let mut interpreter = Interpreter::default();

        let expr = r#"
           let x = 1;

           match x {
                1 => ok(1),
                2 => err("none")
           }
        "#;

        let expr = Expr::from_text(expr).unwrap();
        let compiler = RibCompiler::default();
        let compiled = compiler.compile(expr).unwrap();
        let rib_result = interpreter.run(compiled.byte_code).await.unwrap();

        let expected = ValueAndType::new(
            Value::Result(Ok(Some(Box::new(Value::S32(1))))),
            result(s32(), str()),
        );

        assert_eq!(rib_result.get_val().unwrap(), expected);
    }

    #[test]
    async fn test_interpreter_pattern_match_dynamic_branch_2() {
        let mut interpreter = Interpreter::default();

        let expr = r#"
           let x = some({foo: 1});

           match x {
               some(x) => ok(x.foo),
               none => err("none")
           }
        "#;

        let expr = Expr::from_text(expr).unwrap();
        let compiler = RibCompiler::default();
        let compiled = compiler.compile(expr).unwrap();
        let rib_result = interpreter.run(compiled.byte_code).await.unwrap();

        let expected = ValueAndType::new(
            Value::Result(Ok(Some(Box::new(Value::S32(1))))),
            result(s32(), str()),
        );

        assert_eq!(rib_result.get_val().unwrap(), expected);
    }

    #[test]
    async fn test_interpreter_pattern_match_on_tuple_with_all_types() {
        let mut interpreter = Interpreter::default();

        let tuple = test_utils::get_analysed_type_tuple();

        let analysed_exports = test_utils::configurable_metadata("foo", vec![tuple], Some(str()));

        let expr = r#"
           let worker = instance();
           let record = { request : { path : { user : "jak" } }, y : "bar" };
           let input = (1, ok(100), "bar", record, process-user("jon"), register-user(1u64), validate, prod, dev, test);
           worker.foo(input);
           match input {
             (n1, err(x1), txt, rec, process-user(x), register-user(n), validate, dev, prod, test) =>  "Invalid",
             (n1, ok(x2), txt, rec, process-user(x), register-user(n), validate, prod, dev, test) =>  "foo ${x2} ${n1} ${txt} ${rec.request.path.user} ${validate} ${prod} ${dev} ${test}"
           }

        "#;

        let expr = Expr::from_text(expr).unwrap();
        let compiler = RibCompiler::new(RibCompilerConfig::new(analysed_exports, vec![]));
        let compiled = compiler.compile(expr).unwrap();
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
            test_utils::configurable_metadata("my-worker-function", vec![tuple], Some(str()));

        let expr = r#"
           let worker = instance();
           let record = { request : { path : { user : "jak" } }, y : "baz" };
           let input = (1, ok(1), "bar", record, process-user("jon"), register-user(1u64), validate, prod, dev, test);
           worker.my-worker-function(input);
           match input {
             (n1, ok(x), txt, rec, _, _, _, _, prod, _) =>  "prod ${n1} ${txt} ${rec.request.path.user} ${rec.y}",
             (n1, ok(x), txt, rec, _, _, _, _, dev, _) =>   "dev ${n1} ${txt} ${rec.request.path.user} ${rec.y}"
           }
        "#;

        let expr = Expr::from_text(expr).unwrap();
        let compiler = RibCompiler::new(RibCompilerConfig::new(analysed_exports, vec![]));
        let compiled = compiler.compile(expr).unwrap();
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

        let mut interpreter =
            test_utils::interpreter_with_static_function_response(&result_value, None);

        let analysed_exports = test_utils::configurable_metadata(
            "my-worker-function",
            vec![input_analysed_type],
            Some(output_analysed_type),
        );

        let expr = r#"
           let worker = instance();
           let input = { request : { path : { user : "jak" } }, y : "baz" };
           let result = worker.my-worker-function(input);
           match result {
             ok(result) => { body: result, status: 200 },
             err(result) => { status: 400, body: 400 }
           }
        "#;

        let expr = Expr::from_text(expr).unwrap();
        let compiler = RibCompiler::new(RibCompilerConfig::new(analysed_exports, vec![]));
        let compiled = compiler.compile(expr).unwrap();
        let result = interpreter.run(compiled.byte_code).await.unwrap();

        let expected = test_utils::get_value_and_type(
            &record(vec![field("body", u64()), field("status", s32())]),
            r#"{body: 1, status: 200}"#,
        );

        assert_eq!(result.get_val().unwrap(), expected);
    }

    #[test]
    async fn test_interpreter_tuple_output_in_pattern_match() {
        let input_analysed_type = test_utils::get_analysed_type_record();
        let output_analysed_type = test_utils::get_analysed_type_result();

        let result_value = get_value_and_type(&output_analysed_type, r#"err("failed")"#);

        let mut interpreter =
            test_utils::interpreter_with_static_function_response(&result_value, None);

        let analysed_exports = test_utils::configurable_metadata(
            "my-worker-function",
            vec![input_analysed_type],
            Some(output_analysed_type),
        );

        let expr = r#"
           let input = { request : { path : { user : "jak" } }, y : "baz" };
           let worker = instance();
           let result = worker.my-worker-function(input);
           match result {
             ok(res) => ("${res}", "foo"),
             err(msg) => (msg, "bar")
           }
        "#;

        let expr = Expr::from_text(expr).unwrap();
        let compiler = RibCompiler::new(RibCompilerConfig::new(analysed_exports, vec![]));
        let compiled = compiler.compile(expr).unwrap();
        let result = interpreter.run(compiled.byte_code).await.unwrap();

        let expected = get_value_and_type(&tuple(vec![str(), str()]), r#"("failed", "bar")"#);

        assert_eq!(result.get_val().unwrap(), expected);
    }

    #[test]
    async fn test_interpreter_with_indexed_resource_drop() {
        let expr = r#"
           let user_id = "user";
           let worker = instance();
           let cart = worker.cart(user_id);
           cart.drop();
           "success"
        "#;
        let expr = Expr::from_text(expr).unwrap();
        let component_metadata = test_utils::get_metadata_with_resource_with_params();

        let compiler_config = RibCompilerConfig::new(component_metadata, vec![]);
        let compiler = RibCompiler::new(compiler_config);
        let compiled = compiler.compile(expr).unwrap();

        let mut rib_interpreter = test_utils::interpreter_with_resource_function_invoke_impl(None);
        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(result.get_val().unwrap(), "success".into_value_and_type());
    }

    #[test]
    async fn test_interpreter_with_indexed_resource_checkout() {
        let expr = r#"
           let user_id = "foo";
           let worker = instance();
           let cart = worker.cart(user_id);
           let result = cart.checkout();
           result
        "#;

        let expr = Expr::from_text(expr).unwrap();

        let test_deps = RibTestDeps::test_deps_with_indexed_resource_functions(None);

        let compiler_config =
            RibCompilerConfig::new(test_deps.component_dependencies.clone(), vec![]);
        let compiler = RibCompiler::new(compiler_config);

        let compiled = compiler.compile(expr).unwrap();

        let mut rib_executor = test_deps.interpreter;
        let result = rib_executor.run(compiled.byte_code).await.unwrap();

        let expected_value = Value::Variant {
            case_idx: 1,
            case_value: Some(Box::new(Value::Record(vec![Value::String(
                "foo".to_string(),
            )]))),
        };

        assert_eq!(result.get_val().unwrap().value, expected_value);
    }

    #[test]
    async fn test_interpreter_with_indexed_resources_static_functions_1() {
        let expr = r#"
           let worker = instance();
           let result = worker.cart.create("afsal");
           result.checkout()
        "#;

        let expr = Expr::from_text(expr).unwrap();

        let test_deps = RibTestDeps::test_deps_with_indexed_resource_functions(None);

        let compiler_config =
            RibCompilerConfig::new(test_deps.component_dependencies.clone(), vec![]);
        let compiler = RibCompiler::new(compiler_config);

        let compiled = compiler.compile(expr).unwrap();

        let mut rib_executor = test_deps.interpreter;
        let result = rib_executor.run(compiled.byte_code).await.unwrap();

        let expected_value = Value::Variant {
            case_idx: 1,
            case_value: Some(Box::new(Value::Record(vec![Value::String(
                "foo".to_string(),
            )]))),
        };

        assert_eq!(result.get_val().unwrap().value, expected_value);
    }

    #[test]
    async fn test_interpreter_with_indexed_resources_static_functions_2() {
        let expr = r#"
           let worker = instance();
           let default-cart = worker.cart("default");
           let alternate-cart = worker.cart.create-safe("afsal");
           match alternate-cart {
             ok(alt) => alt.checkout(),
             err(_) => default-cart.checkout()
           }
        "#;

        let expr = Expr::from_text(expr).unwrap();

        let test_deps = RibTestDeps::test_deps_with_indexed_resource_functions(None);

        let compiler_config =
            RibCompilerConfig::new(test_deps.component_dependencies.clone(), vec![]);
        let compiler = RibCompiler::new(compiler_config);

        let compiled = compiler.compile(expr).unwrap();

        let mut rib_executor = test_deps.interpreter;
        let result = rib_executor.run(compiled.byte_code).await.unwrap();

        let expected_value = Value::Variant {
            case_idx: 1,
            case_value: Some(Box::new(Value::Record(vec![Value::String(
                "foo".to_string(),
            )]))),
        };

        assert_eq!(result.get_val().unwrap().value, expected_value);
    }

    #[test]
    async fn test_interpreter_with_indexed_resource_get_cart_contents() {
        let expr = r#"
           let user_id = "bar";
           let worker = instance();
           let cart = worker.cart(user_id);
           let result = cart.get-cart-contents();
           result[0].product-id
        "#;

        let expr = Expr::from_text(expr).unwrap();

        let test_deps = RibTestDeps::test_deps_with_indexed_resource_functions(None);

        let compiler_config =
            RibCompilerConfig::new(test_deps.component_dependencies.clone(), vec![]);
        let compiler = RibCompiler::new(compiler_config);

        let compiled = compiler.compile(expr).unwrap();

        let mut rib_executor = test_deps.interpreter;

        let result = rib_executor.run(compiled.byte_code).await.unwrap();

        assert_eq!(result.get_val().unwrap(), "foo".into_value_and_type());
    }

    #[test]
    async fn test_interpreter_with_indexed_resource_update_item_quantity() {
        let expr = r#"
           let user_id = "jon";
           let product_id = "mac";
           let quantity = 1032;
           let worker = instance();
           let cart = worker.cart(user_id);
           cart.update-item-quantity(product_id, quantity);
           "successfully updated"
        "#;
        let expr = Expr::from_text(expr).unwrap();

        let test_deps = RibTestDeps::test_deps_with_indexed_resource_functions(None);

        let compiler_config =
            RibCompilerConfig::new(test_deps.component_dependencies.clone(), vec![]);
        let compiler = RibCompiler::new(compiler_config);

        let compiled = compiler.compile(expr).unwrap();

        let mut rib_executor = test_deps.interpreter;

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
           let worker = instance();
           let cart = worker.cart(user_id);
           cart.add-item(product);

           "successfully added"
        "#;

        let expr = Expr::from_text(expr).unwrap();

        let test_deps = RibTestDeps::test_deps_with_indexed_resource_functions(None);

        let compiler_config =
            RibCompilerConfig::new(test_deps.component_dependencies.clone(), vec![]);
        let compiler = RibCompiler::new(compiler_config);

        let compiled = compiler.compile(expr).unwrap();

        let mut rib_executor = test_deps.interpreter;

        let result = rib_executor.run(compiled.byte_code).await.unwrap();

        assert_eq!(
            result.get_val().unwrap(),
            "successfully added".into_value_and_type()
        );
    }

    #[test]
    async fn test_interpreter_with_resource_add_item() {
        let expr = r#"
           let worker = instance();
           let cart = worker.cart();
           let user_id = "foo";
           let product = { product-id: "mac", name: "macbook", quantity: 1u32, price: 1f32 };
           cart.add-item(product);

           "successfully added"
        "#;

        let expr = Expr::from_text(expr).unwrap();

        let test_deps = RibTestDeps::test_deps_with_resource_functions(None);

        let compiler_config =
            RibCompilerConfig::new(test_deps.component_dependencies.clone(), vec![]);
        let compiler = RibCompiler::new(compiler_config);

        let compiled = compiler.compile(expr).unwrap();

        let mut rib_executor = test_deps.interpreter;

        let result = rib_executor.run(compiled.byte_code).await.unwrap();

        assert_eq!(
            result.get_val().unwrap(),
            "successfully added".into_value_and_type()
        );
    }

    #[test]
    async fn test_interpreter_with_resource_get_cart_contents() {
        let expr = r#"
           let worker = instance();
           let cart = worker.cart();
           let result = cart.get-cart-contents();
           result[0].product-id
        "#;

        let expr = Expr::from_text(expr).unwrap();

        let test_deps = RibTestDeps::test_deps_with_resource_functions(None);

        let compiler_config =
            RibCompilerConfig::new(test_deps.component_dependencies.clone(), vec![]);
        let compiler = RibCompiler::new(compiler_config);

        let compiled = compiler.compile(expr).unwrap();

        let mut rib_executor = test_deps.interpreter;
        let result = rib_executor.run(compiled.byte_code).await.unwrap();

        assert_eq!(result.get_val().unwrap(), "foo".into_value_and_type());
    }

    #[test]
    async fn test_interpreter_with_resource_update_item() {
        let expr = r#"
           let worker = instance();
           let product_id = "mac";
           let quantity = 1032;
           let cart = worker.cart();
           cart.update-item-quantity(product_id, quantity);
           "successfully updated"
        "#;
        let expr = Expr::from_text(expr).unwrap();

        let test_deps = RibTestDeps::test_deps_with_resource_functions(None);

        let compiler_config =
            RibCompilerConfig::new(test_deps.component_dependencies.clone(), vec![]);
        let compiler = RibCompiler::new(compiler_config);

        let compiled = compiler.compile(expr).unwrap();

        let mut rib_executor = test_deps.interpreter;

        let result = rib_executor.run(compiled.byte_code).await.unwrap();

        assert_eq!(
            result.get_val().unwrap(),
            "successfully updated".into_value_and_type()
        );
    }

    #[test]
    async fn test_interpreter_with_resource_checkout() {
        let expr = r#"
           let worker = instance();
           let cart = worker.cart();
           let result = cart.checkout();
           result
        "#;

        let expr = Expr::from_text(expr).unwrap();

        let test_deps = RibTestDeps::test_deps_with_resource_functions(None);

        let compiler_config =
            RibCompilerConfig::new(test_deps.component_dependencies.clone(), vec![]);

        let compiler = RibCompiler::new(compiler_config);

        let compiled = compiler.compile(expr).unwrap();

        let mut interpreter = test_deps.interpreter;

        let result = interpreter.run(compiled.byte_code).await.unwrap();

        let expected_result = Value::Variant {
            case_idx: 1,
            case_value: Some(Box::new(Value::Record(vec![Value::String(
                "foo".to_string(),
            )]))),
        };

        assert_eq!(result.get_val().unwrap().value, expected_result);
    }

    #[test]
    async fn test_interpreter_with_resource_drop() {
        let expr = r#"
           let worker = instance();
           let cart = worker.cart();
           cart.drop();
           "success"
        "#;
        let expr = Expr::from_text(expr).unwrap();
        let test_deps = RibTestDeps::test_deps_with_resource_functions(None);

        let compiler_config =
            RibCompilerConfig::new(test_deps.component_dependencies.clone(), vec![]);
        let compiler = RibCompiler::new(compiler_config);
        let compiled = compiler.compile(expr).unwrap();

        let mut rib_interpreter = test_deps.interpreter;
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

        let compiler = RibCompiler::default();
        let compiled = compiler.compile(expr).unwrap();

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

        let compiler = RibCompiler::default();
        let compiled = compiler.compile(expr).unwrap();

        let mut interpreter = Interpreter::default();
        let result = interpreter
            .run(compiled.byte_code)
            .await
            .unwrap_err()
            .to_string();

        assert_eq!(result, "index out of bound: 10 (size: 5)".to_string());
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

        let compiler = RibCompiler::default();
        let compiled = compiler.compile(expr).unwrap();

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

        let compiler = RibCompiler::default();
        let compiled = compiler.compile(expr).unwrap();

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

        let compiler = RibCompiler::default();
        let compiled = compiler.compile(expr).unwrap();

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

        let compiler = RibCompiler::default();
        let compiled = compiler.compile(expr).unwrap();

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

        let compiler = RibCompiler::default();
        let compiled = compiler.compile(expr).unwrap();

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

        let compiler = RibCompiler::default();
        let compiled = compiler.compile(expr).unwrap();

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

        let compiler = RibCompiler::default();
        let compiled = compiler.compile(expr).unwrap();

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

        let compiler = RibCompiler::default();
        let compiled = compiler.compile(expr).unwrap();

        let mut interpreter = Interpreter::default();
        let result = interpreter.run(compiled.byte_code).await.unwrap();

        let expected = ValueAndType::new(
            Value::Record(vec![
                Value::S32(1),
                Value::Bool(false), // non inclusive
            ]),
            record(vec![field("from", s32()), field("inclusive", bool())]),
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

        let compiler = RibCompiler::default();
        let compiled = compiler.compile(expr).unwrap();

        let mut interpreter = Interpreter::default();
        let result = interpreter.run(compiled.byte_code).await.unwrap();

        let expected = ValueAndType::new(
            Value::Record(vec![
                Value::S32(1),
                Value::S32(2),
                Value::Bool(false), // non inclusive
            ]),
            record(vec![
                field("from", s32()),
                field("to", s32()),
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

        let compiler = RibCompiler::default();
        let compiled = compiler.compile(expr).unwrap();

        let mut interpreter = Interpreter::default();
        let result = interpreter.run(compiled.byte_code).await.unwrap();

        let expected = ValueAndType::new(
            Value::Record(vec![
                Value::S32(1),
                Value::S32(10),
                Value::Bool(true), // inclusive
            ]),
            record(vec![
                field("from", s32()),
                field("to", s32()),
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

        let compiler = RibCompiler::default();
        let compiled = compiler.compile(expr).unwrap();

        let mut interpreter = Interpreter::default();
        let result = interpreter.run(compiled.byte_code).await.unwrap();

        let expected = ValueAndType::new(
            Value::Record(vec![Value::U64(1), Value::U64(1), Value::Bool(false)]),
            record(vec![
                field("from", u64()),
                field("to", u64()),
                field("inclusive", bool()),
            ]),
        );

        assert_eq!(result.get_val().unwrap(), expected);
    }

    #[test]
    async fn test_interpreter_range_returns_5() {
        let expr = r#"
              let y = 1 + 10;
              1..y
              "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiler = RibCompiler::default();
        let compiled = compiler.compile(expr).unwrap();

        let mut interpreter = Interpreter::default();
        let result = interpreter.run(compiled.byte_code).await.unwrap();

        let expected = ValueAndType::new(
            Value::Record(vec![Value::S32(1), Value::S32(11), Value::Bool(false)]),
            record(vec![
                field("from", s32()),
                field("to", s32()),
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

        let compiler = RibCompiler::default();
        let compiled = compiler.compile(expr).unwrap();

        let mut interpreter = Interpreter::default();
        let result = interpreter.run(compiled.byte_code).await.unwrap();

        let expected = ValueAndType::new(
            Value::List(vec![
                Value::S32(1),
                Value::S32(2),
                Value::S32(3),
                Value::S32(4),
                Value::S32(5),
            ]),
            list(s32()),
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

        let compiler = RibCompiler::default();
        let compiled = compiler.compile(expr).unwrap();

        let mut interpreter = Interpreter::default();
        let result = interpreter.run(compiled.byte_code).await.unwrap();

        let expected = ValueAndType::new(
            Value::List(vec![
                Value::S32(1),
                Value::S32(2),
                Value::S32(3),
                Value::S32(4),
            ]),
            list(s32()),
        );

        assert_eq!(result.get_val().unwrap(), expected);
    }

    #[test]
    async fn test_interpreter_range_with_comprehension_3() {
        // infinite computation will respond with an error - than a stack overflow
        // Note that, `list[1..]` is allowed while `for i in 1.. { yield i; }` is not
        let expr = r#"
              let range = 1..;
              for i in range {
                yield i;
              }

              "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiler = RibCompiler::default();
        let compiled = compiler.compile(expr).unwrap();

        let mut interpreter = Interpreter::default();
        let result = interpreter.run(compiled.byte_code).await;
        assert!(result.is_err());
    }

    #[test]
    async fn test_interpreter_range_with_list_reduce_1() {
        // infinite computation will respond with an error - than a stack overflow
        // Note that, `list[1..]` is allowed while `for i in 1.. { yield i; }` is not
        let expr = r#"
                let initial = 1;
                let final = 5;
                let x = initial..final;

                reduce z, a in x from 0u8 {
                  yield z + a;
                }

              "#;

        let expr = Expr::from_text(expr).unwrap();

        let compiler = RibCompiler::default();
        let compiled = compiler.compile(expr).unwrap();

        let mut interpreter = Interpreter::default();
        let result = interpreter.run(compiled.byte_code).await.unwrap();

        let expected = ValueAndType::new(Value::U8(10), u8());

        assert_eq!(result.get_val().unwrap(), expected);
    }

    #[test]
    async fn test_interpreter_ephemeral_worker_0() {
        let expr = r#"
              let x = instance();
              let result = x.pass-through(1, 2);
              result
            "#;
        let expr = Expr::from_text(expr).unwrap();

        let test_deps = RibTestDeps::test_deps_for_pass_through_function();

        let compiler_config =
            RibCompilerConfig::new(test_deps.component_dependencies.clone(), vec![]);
        let compiler = RibCompiler::new(compiler_config);
        let compiled = compiler.compile(expr).unwrap();

        let mut rib_interpreter = test_deps.interpreter;

        let result = rib_interpreter
            .run(compiled.byte_code)
            .await
            .unwrap()
            .get_val()
            .unwrap()
            .value;

        let expected_value = Value::Record(vec![
            Value::String("test-worker".to_string()),
            Value::String("pass-through".to_string()),
            Value::U64(1),
            Value::U32(2),
        ]);

        assert_eq!(result, expected_value)
    }

    #[test]
    async fn test_interpreter_ephemeral_worker_1() {
        let expr = r#"
              let x = instance();
              x
            "#;
        let expr = Expr::from_text(expr).unwrap();

        let test_deps = RibTestDeps::test_deps_with_global_functions();

        let compiler = RibCompiler::new(RibCompilerConfig::new(
            test_deps.component_dependencies.clone(),
            vec![],
        ));

        let compiled = compiler.compile(expr);

        assert!(compiled.is_ok());
    }

    #[test]
    async fn test_interpreter_ephemeral_worker_2() {
        let expr = r#"
             instance
            "#;
        let expr = Expr::from_text(expr).unwrap();

        let test_deps = RibTestDeps::test_deps_with_multiple_interfaces(None);

        let compiler = RibCompiler::new(RibCompilerConfig::new(
            test_deps.component_dependencies.clone(),
            vec![],
        ));

        let compiled = compiler.compile(expr);

        assert!(compiled.is_err());
    }

    #[test]
    async fn test_interpreter_ephemeral_worker_3() {
        let expr = r#"
              instance()
            "#;
        let expr = Expr::from_text(expr).unwrap();

        let test_deps = RibTestDeps::test_deps_with_multiple_interfaces(None);

        let compiler = RibCompiler::new(RibCompilerConfig::new(
            test_deps.component_dependencies.clone(),
            vec![],
        ));

        let compiled = compiler.compile(expr);

        assert!(compiled.is_ok());
    }

    #[test]
    async fn test_interpreter_ephemeral_worker_4() {
        let expr = r#"
              let worker = instance().foo("bar")
            "#;
        let expr = Expr::from_text(expr).unwrap();

        let test_deps = RibTestDeps::test_deps_with_multiple_interfaces(None);

        let compiler_config =
            RibCompilerConfig::new(test_deps.component_dependencies.clone(), vec![]);
        let compiler = RibCompiler::new(compiler_config);
        let error = compiler.compile(expr).unwrap_err();

        assert_eq!(
            error.to_string(),
            "inline invocation of functions on a worker instance is currently not supported"
        );
    }

    #[test]
    async fn test_interpreter_ephemeral_worker_5() {
        let expr = r#"
              let result = instance.foo("bar");
              result
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let test_deps = RibTestDeps::test_deps_with_multiple_interfaces(None);

        let compiler = RibCompiler::new(RibCompilerConfig::new(
            test_deps.component_dependencies.clone(),
            vec![],
        ));

        let compiled = compiler.compile(expr).unwrap_err().to_string();

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
        let test_deps = RibTestDeps::test_deps_with_multiple_interfaces(None);

        let compiler = RibCompiler::new(RibCompilerConfig::new(
            test_deps.component_dependencies.clone(),
            vec![],
        ));

        let compilation_error = compiler.compile(expr).unwrap_err().to_string();

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
        let test_deps = RibTestDeps::test_deps_with_multiple_interfaces(None);

        let compiler_config =
            RibCompilerConfig::new(test_deps.component_dependencies.clone(), vec![]);
        let compiler = RibCompiler::new(compiler_config);
        let compiled = compiler.compile(expr).unwrap();

        let mut rib_interpreter = test_deps.interpreter;

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(result.get_val().unwrap(), "success".into_value_and_type());
    }

    /// Durable worker
    #[test]
    async fn test_interpreter_durable_worker_0() {
        let expr = r#"
                let worker = instance("my-worker");
                let result = worker.pass-through(42, 43);
                result
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let test_deps = RibTestDeps::test_deps_for_pass_through_function();

        let compiler_config =
            RibCompilerConfig::new(test_deps.component_dependencies.clone(), vec![]);
        let compiler = RibCompiler::new(compiler_config);
        let compiled = compiler.compile(expr).unwrap();

        let mut rib_interpreter = test_deps.interpreter;

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        let expected_val = Value::Record(vec![
            Value::String("my-worker".to_string()),
            Value::String("pass-through".to_string()),
            Value::U64(42),
            Value::U32(43),
        ]);

        assert_eq!(result.get_val().unwrap().value, expected_val);
    }

    #[test]
    async fn test_interpreter_durable_worker_1_1() {
        let expr = r#"
                let x = 1;
                let y = 2;
                let inst = instance("my-worker");
                inst.foo-number(x, y)
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let test_deps = RibTestDeps::test_deps_with_multiple_interfaces(None);

        let compiler_config =
            RibCompilerConfig::new(test_deps.component_dependencies.clone(), vec![]);
        let compiler = RibCompiler::new(compiler_config);
        let compiled = compiler.compile(expr).unwrap();

        let mut rib_interpreter = test_deps.interpreter;

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(result.get_val().unwrap().value, Value::S32(1));
    }

    #[test]
    async fn test_interpreter_durable_worker_2() {
        let expr = r#"
                let inst = instance("my-worker");
                let result = inst.foo("bar");
                result
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let test_deps = RibTestDeps::test_deps_with_multiple_interfaces(None);

        let compiler_config =
            RibCompilerConfig::new(test_deps.component_dependencies.clone(), vec![]);
        let compiler = RibCompiler::new(compiler_config);
        let compiled = compiler.compile(expr).unwrap();

        let mut rib_interpreter = test_deps.interpreter;

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(
            result.get_val().unwrap().value,
            Value::String("foo".to_string())
        );
    }

    #[test]
    async fn test_interpreter_durable_worker_3() {
        let expr = r#"
                let my_worker = instance("my-worker");
                let result = my_worker.foo[api1]("bar");
                result
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let test_deps = RibTestDeps::test_deps_with_multiple_interfaces(None);

        let compiler_config =
            RibCompilerConfig::new(test_deps.component_dependencies.clone(), vec![]);
        let compiler = RibCompiler::new(compiler_config);
        let compiled = compiler.compile(expr).unwrap();

        let mut rib_interpreter = test_deps.interpreter;

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(
            result.get_val().unwrap().value,
            Value::String("foo".to_string())
        );
    }

    #[test]
    async fn test_interpreter_durable_worker_4() {
        let expr = r#"
                let worker = instance("my-worker");
                let result = worker.bar("bar");
                result
            "#;
        let expr = Expr::from_text(expr).unwrap();

        let test_deps = RibTestDeps::test_deps_with_multiple_interfaces(None);

        let compiler = RibCompiler::new(RibCompilerConfig::new(
            test_deps.component_dependencies,
            vec![],
        ));

        let compilation_error = compiler.compile(expr).unwrap_err().to_string();

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
        let test_deps = RibTestDeps::test_deps_with_multiple_interfaces(None);

        let compiler_config =
            RibCompilerConfig::new(test_deps.component_dependencies.clone(), vec![]);
        let compiler = RibCompiler::new(compiler_config);
        let compiled = compiler.compile(expr).unwrap();

        let mut rib_interpreter = test_deps.interpreter;

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(
            result.get_val().unwrap().value,
            Value::String("api1-bar".to_string())
        );
    }

    #[test]
    async fn test_interpreter_durable_worker_6() {
        let expr = r#"
                let worker = instance("my-worker");
                let result = worker.bar[api2]("bar");
                result
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let test_deps = RibTestDeps::test_deps_with_multiple_interfaces(None);

        let compiler_config =
            RibCompilerConfig::new(test_deps.component_dependencies.clone(), vec![]);
        let compiler = RibCompiler::new(compiler_config);
        let compiled = compiler.compile(expr).unwrap();

        let mut rib_interpreter = test_deps.interpreter;

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(
            result.get_val().unwrap().value,
            Value::String("api2-bar".to_string())
        );
    }

    #[test]
    async fn test_interpreter_durable_worker_7() {
        let expr = r#"
                let worker = instance("my-worker");
                let result = worker.baz("bar");
                result
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let test_deps = RibTestDeps::test_deps_with_multiple_interfaces(None);

        let compiler_config = RibCompilerConfig::new(test_deps.component_dependencies, vec![]);
        let compiler = RibCompiler::new(compiler_config);
        let compiled = compiler.compile(expr).unwrap();

        let mut rib_interpreter = test_deps.interpreter;

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(
            result.get_val().unwrap().value,
            Value::String("clock-baz".to_string())
        );
    }

    #[test]
    async fn test_interpreter_durable_worker_8() {
        let expr = r#"
                let worker = instance("my-worker");
                let result = worker.qux("bar");
                result
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let test_deps = RibTestDeps::test_deps_with_multiple_interfaces(None);

        let compiler = RibCompiler::new(RibCompilerConfig::new(
            test_deps.component_dependencies.clone(),
            vec![],
        ));

        let compiled = compiler.compile(expr).unwrap_err().to_string();

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
        let test_deps = RibTestDeps::test_deps_with_multiple_interfaces(None);

        let compiler_config =
            RibCompilerConfig::new(test_deps.component_dependencies.clone(), vec![]);
        let compiler = RibCompiler::new(compiler_config);
        let compiled = compiler.compile(expr).unwrap();

        let mut rib_interpreter = test_deps.interpreter;

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(
            result.get_val().unwrap().value,
            Value::String("qux".to_string())
        );
    }

    #[test]
    async fn test_interpreter_durable_worker_10() {
        let expr = r#"
                let worker = instance("my-worker");
                let result = worker.qux[wasi:clocks]("bar");
                result
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let test_deps = RibTestDeps::test_deps_with_multiple_interfaces(None);

        let compiler_config =
            RibCompilerConfig::new(test_deps.component_dependencies.clone(), vec![]);
        let compiler = RibCompiler::new(compiler_config);
        let compiled = compiler.compile(expr).unwrap();

        let mut rib_interpreter = test_deps.interpreter;

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(
            result.get_val().unwrap().value,
            Value::String("clock-qux".to_string())
        );
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
        let test_deps = RibTestDeps::test_deps_with_multiple_interfaces(None);

        let compiler_config =
            RibCompilerConfig::new(test_deps.component_dependencies.clone(), vec![]);
        let compiler = RibCompiler::new(compiler_config);
        let compiled = compiler.compile(expr).unwrap();

        let mut rib_interpreter = test_deps.interpreter;

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(result.get_val().unwrap(), "success".into_value_and_type());
    }

    #[test]
    async fn test_interpreter_durable_worker_12() {
        let expr = r#"
                let worker = instance("my-worker");
                for i in [1, 2, 3] {
                   worker.foo("${i}");
                   yield i;
                }
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let test_deps = RibTestDeps::test_deps_with_multiple_interfaces(None);

        let compiler_config =
            RibCompilerConfig::new(test_deps.component_dependencies.clone(), vec![]);
        let compiler = RibCompiler::new(compiler_config);
        let compiled = compiler.compile(expr).unwrap();

        let mut rib_interpreter = test_deps.interpreter;

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(
            result.get_val().unwrap().value,
            Value::List(vec![Value::S32(1), Value::S32(2), Value::S32(3)])
        );
    }

    #[test]
    async fn test_interpreter_durable_worker_with_resource_0() {
        let expr = r#"
                let worker = instance("my-worker");
                worker.cart[golem:it]("bar")
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let test_deps = RibTestDeps::test_deps_with_indexed_resource_functions(None);

        let compiler = RibCompiler::new(RibCompilerConfig::new(
            test_deps.component_dependencies.clone(),
            vec![],
        ));

        let compiled = compiler.compile(expr);

        assert!(compiled.is_ok());
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
        let test_deps = RibTestDeps::test_deps_with_indexed_resource_functions(None);

        let compiler_config =
            RibCompilerConfig::new(test_deps.component_dependencies.clone(), vec![]);
        let compiler = RibCompiler::new(compiler_config);
        let compiled = compiler.compile(expr).unwrap();

        let mut rib_interpreter = test_deps.interpreter;

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(result.get_val().unwrap(), "success".into_value_and_type());
    }

    #[test]
    async fn test_interpreter_durable_worker_with_resource_2() {
        let expr = r#"
                let worker = instance("my-worker");
                let cart = worker.cart[golem:it]("bar");
                let result = cart.add-item({product-id: "mac", name: "macbook", price: 1:f32, quantity: 1:u32});
                result
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let test_deps = RibTestDeps::test_deps_with_indexed_resource_functions(None);

        let compiler_config = RibCompilerConfig::new(test_deps.component_dependencies, vec![]);
        let compiler = RibCompiler::new(compiler_config);
        let compiled = compiler.compile(expr).unwrap();

        let mut rib_interpreter = test_deps.interpreter;

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(result, RibResult::Unit);
    }

    #[test]
    async fn test_interpreter_durable_worker_with_resource_3() {
        let expr = r#"
                let worker = instance("my-worker");
                let cart = worker.cart[golem:it]("bar");
                cart.add-items({product-id: "mac", name: "macbook", price: 1:f32, quantity: 1:u32});
                "success"
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let test_deps = RibTestDeps::test_deps_with_indexed_resource_functions(None);

        let compiler = RibCompiler::new(RibCompilerConfig::new(
            test_deps.component_dependencies.clone(),
            vec![],
        ));

        let compiled = compiler.compile(expr).unwrap_err().to_string();

        assert_eq!(compiled, "error in the following rib found at line 4, column 17\n`cart.add-items({product-id: \"mac\", name: \"macbook\", price: 1: f32, quantity: 1: u32})`\ncause: invalid function call `add-items`\nfunction 'add-items' not found\n".to_string());
    }

    #[test]
    async fn test_interpreter_durable_worker_with_resource_4() {
        let expr = r#"
                let worker = instance("my-worker");
                let cart = worker.carts[golem:it]("bar");
                cart.add-item({product-id: "mac", name: "macbook", price: 1:f32, quantity: 1:u32});
                "success"
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let test_deps = RibTestDeps::test_deps_with_indexed_resource_functions(None);

        let compiler = RibCompiler::new(RibCompilerConfig::new(
            test_deps.component_dependencies.clone(),
            vec![],
        ));

        let compiled = compiler.compile(expr).unwrap_err().to_string();

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
                cart.add-item({product-id: "mac", name: "macbook", price: 1, quantity: 1});
                "success"
            "#;
        let expr = Expr::from_text(expr).unwrap();
        let test_deps = RibTestDeps::test_deps_with_indexed_resource_functions(None);

        let compiler_config =
            RibCompilerConfig::new(test_deps.component_dependencies.clone(), vec![]);
        let compiler = RibCompiler::new(compiler_config);
        let compiled = compiler.compile(expr).unwrap();

        let mut rib_interpreter = test_deps.interpreter;

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

        let test_deps = RibTestDeps::test_deps_with_indexed_resource_functions(None);

        let compiler = RibCompiler::new(RibCompilerConfig::new(
            test_deps.component_dependencies.clone(),
            vec![],
        ));

        let error_message = compiler.compile(expr).unwrap_err().to_string();

        let expected = r#"
            error in the following rib found at line 4, column 57
            `1`
            cause: type mismatch. expected string, found s32
            the expression `1` is inferred as `s32` by default
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
        let test_deps = RibTestDeps::test_deps_with_indexed_resource_functions(None);

        let compiler_config =
            RibCompilerConfig::new(test_deps.component_dependencies.clone(), vec![]);
        let compiler = RibCompiler::new(compiler_config);
        let compiled = compiler.compile(expr).unwrap();

        let mut rib_interpreter = test_deps.interpreter;

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
        let test_deps = RibTestDeps::test_deps_with_indexed_resource_functions(None);
        let compiler_config =
            RibCompilerConfig::new(test_deps.component_dependencies.clone(), vec![]);
        let compiler = RibCompiler::new(compiler_config);
        let compiled = compiler.compile(expr).unwrap();

        let mut rib_interpreter = test_deps.interpreter;

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
        let test_deps = RibTestDeps::test_deps_with_indexed_resource_functions(None);
        let compiler_config =
            RibCompilerConfig::new(test_deps.component_dependencies.clone(), vec![]);
        let compiler = RibCompiler::new(compiler_config);
        let compiled = compiler.compile(expr).unwrap();

        let mut rib_interpreter = test_deps.interpreter;

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        let expected_value = Value::List(vec![Value::Record(vec![
            Value::String("foo".to_string()),
            Value::String("bar".to_string()),
            Value::F32(10.0),
            Value::U32(2),
        ])]);

        assert_eq!(result.get_val().unwrap().value, expected_value);
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
        let test_deps = RibTestDeps::test_deps_with_indexed_resource_functions(None);

        let compiler_config =
            RibCompilerConfig::new(test_deps.component_dependencies.clone(), vec![]);
        let compiler = RibCompiler::new(compiler_config);
        let compiled = compiler.compile(expr).unwrap();

        let mut rib_interpreter = test_deps.interpreter;

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        let expected_value = Value::List(vec![Value::Record(vec![
            Value::String("foo".to_string()),
            Value::String("bar".to_string()),
            Value::F32(10.0),
            Value::U32(2),
        ])]);

        assert_eq!(result.get_val().unwrap().value, expected_value);
    }

    #[test]
    async fn test_interpreter_durable_worker_with_resource_11() {
        let expr = r#"
                let worker = instance(request.path.user-id: string);
                let result = worker.qux[amazon:shopping-cart]("bar");
                result
            "#;
        let expr = Expr::from_text(expr).unwrap();

        let mut input = HashMap::new();

        // Passing request data as input to interpreter
        let rib_input_key = "request";
        let rib_input_value = ValueAndType::new(
            Value::Record(vec![Value::Record(vec![Value::String("user".to_string())])]),
            record(vec![field("path", record(vec![field("user-id", str())]))]),
        );

        input.insert(rib_input_key.to_string(), rib_input_value);

        let rib_input = RibInput::new(input);

        let test_deps = RibTestDeps::test_deps_with_multiple_interfaces(Some(rib_input));

        let compiler_config =
            RibCompilerConfig::new(test_deps.component_dependencies.clone(), vec![]);
        let compiler = RibCompiler::new(compiler_config);
        let compiled = compiler.compile(expr).unwrap();

        let mut rib_interpreter = test_deps.interpreter;

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(
            result.get_val().unwrap().value,
            Value::String("qux".to_string())
        )
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

        let mut input = HashMap::new();

        let rib_input_key = "request";

        let rib_input_value = ValueAndType::new(
            Value::Record(vec![Value::Record(vec![Value::String("user".to_string())])]),
            record(vec![field("path", record(vec![field("user-id", str())]))]),
        );

        input.insert(rib_input_key.to_string(), rib_input_value);

        let rib_input = RibInput::new(input);

        let test_deps = RibTestDeps::test_deps_with_multiple_interfaces(Some(rib_input));

        let compiler_config =
            RibCompilerConfig::new(test_deps.component_dependencies.clone(), vec![]);

        let compiler = RibCompiler::new(compiler_config);

        let compiled = compiler.compile(expr).unwrap();

        let mut rib_interpreter = test_deps.interpreter;

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

        let test_deps = RibTestDeps::test_deps_with_multiple_interfaces(None);

        let compiler = RibCompiler::new(RibCompilerConfig::new(
            test_deps.component_dependencies.clone(),
            vec![],
        ));

        let error = compiler.compile(expr).unwrap_err().to_string();

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

        let test_deps = RibTestDeps::test_deps_with_multiple_interfaces(None);

        let compiler = RibCompiler::new(RibCompilerConfig::new(
            test_deps.component_dependencies.clone(),
            vec![],
        ));

        let error = compiler.compile(expr).unwrap_err().to_string();

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

        let test_deps = RibTestDeps::test_deps_with_multiple_interfaces(None);

        let compiler_config =
            RibCompilerConfig::new(test_deps.component_dependencies.clone(), vec![]);

        let compiler = RibCompiler::new(compiler_config);

        let compiled = compiler.compile(expr).unwrap();

        let mut rib_interpreter = test_deps.interpreter;

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        let result_val = result.get_val().unwrap();

        assert_eq!(result_val.value, Value::String("qux".to_string()));
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

        let mut input = HashMap::new();

        let rib_input_key = "request";

        let rib_input_value = ValueAndType::new(
            Value::Record(vec![Value::Record(vec![Value::String("user".to_string())])]),
            record(vec![field("path", record(vec![field("user-id", str())]))]),
        );

        input.insert(rib_input_key.to_string(), rib_input_value);

        let rib_input = RibInput::new(input);

        let test_deps = RibTestDeps::test_deps_with_indexed_resource_functions(Some(rib_input));

        let compiler_config =
            RibCompilerConfig::new(test_deps.component_dependencies.clone(), vec![]);

        let compiler = RibCompiler::new(compiler_config);

        let compiled = compiler.compile(expr).unwrap();

        let mut rib_interpreter = test_deps.interpreter;

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        let result_val = result.get_val().unwrap();

        let expected_val = Value::List(vec![Value::Record(vec![
            Value::String("foo".to_string()),
            Value::String("bar".to_string()),
            Value::F32(10.0),
            Value::U32(2),
        ])]);

        assert_eq!(result_val.value, expected_val)
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

        let mut input = HashMap::new();

        let rib_input_key = "request";

        let rib_input_value = ValueAndType::new(
            Value::Record(vec![Value::Record(vec![Value::String("user".to_string())])]),
            record(vec![field("path", record(vec![field("user-id", str())]))]),
        );

        input.insert(rib_input_key.to_string(), rib_input_value);

        let rib_input = RibInput::new(input);

        let test_deps = RibTestDeps::test_deps_with_indexed_resource_functions(Some(rib_input));

        let compiler_config =
            RibCompilerConfig::new(test_deps.component_dependencies.clone(), vec![]);

        let compiler = RibCompiler::new(compiler_config);

        let compiled = compiler.compile(expr).unwrap();

        let mut rib_interpreter = test_deps.interpreter;

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        let result_val = result.get_val().unwrap().value;

        let cart_contents = Value::List(vec![Value::Record(vec![
            Value::String("foo".to_string()),
            Value::String("bar".to_string()),
            Value::F32(10.0),
            Value::U32(2),
        ])]);

        let expected_val = Value::List(vec![
            cart_contents.clone(),
            cart_contents.clone(),
            cart_contents,
        ]);

        assert_eq!(result_val, expected_val);
    }

    #[test]
    async fn test_interpreter_durable_worker_with_resource_18() {
        let expr = r#"

            let initial = 1;
            let final = 5;
            let range = initial..final;
            let worker = instance("my-worker");
            let cart = worker.cart[golem:it]("bar");

            for i in range {
                yield cart.add-item(request.body);
            };

            "success"
        "#;
        let expr = Expr::from_text(expr).unwrap();

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

        let test_deps = RibTestDeps::test_deps_with_indexed_resource_functions(Some(rib_input));

        let compiler_config =
            RibCompilerConfig::new(test_deps.component_dependencies.clone(), vec![]);

        let compiler = RibCompiler::new(compiler_config);

        let compiled = compiler.compile(expr).unwrap();

        let mut rib_interpreter = test_deps.interpreter;

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(result.get_val().unwrap(), "success".into_value_and_type());
    }

    #[test]
    async fn test_interpreter_durable_worker_with_resource_19() {
        let expr = r#"

            let initial = 1;
            let final = 5;
            let range = initial..final;

            for i in range {
                let worker = instance("my-worker");
                let cart = worker.cart[golem:it]("bar");
                yield cart.add-item(request.body);
            };

            "success"
        "#;
        let expr = Expr::from_text(expr).unwrap();

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

        let test_deps = RibTestDeps::test_deps_with_indexed_resource_functions(Some(rib_input));

        let compiler_config =
            RibCompilerConfig::new(test_deps.component_dependencies.clone(), vec![]);

        let compiler = RibCompiler::new(compiler_config);

        let compiled = compiler.compile(expr).unwrap();

        let mut rib_interpreter = test_deps.interpreter;

        let result = rib_interpreter.run(compiled.byte_code).await.unwrap();

        assert_eq!(result.get_val().unwrap(), "success".into_value_and_type());
    }

    mod test_utils {
        use crate::interpreter::rib_interpreter::internal::NoopRibFunctionInvoke;
        use crate::interpreter::rib_interpreter::Interpreter;
        use crate::{
            ComponentDependency, ComponentDependencyKey, DefaultWorkerNameGenerator,
            EvaluatedFnArgs, EvaluatedFqFn, EvaluatedWorkerName, GenerateWorkerName,
            GetLiteralValue, InstructionId, RibComponentFunctionInvoke, RibFunctionInvokeResult,
            RibInput,
        };
        use async_trait::async_trait;
        use golem_wasm_ast::analysis::analysed_type::{
            case, f32, field, handle, list, option, r#enum, record, result, s32, str, tuple, u32,
            u64, unit_case, variant,
        };
        use golem_wasm_ast::analysis::{
            AnalysedExport, AnalysedFunction, AnalysedFunctionParameter, AnalysedFunctionResult,
            AnalysedInstance, AnalysedResourceId, AnalysedResourceMode, AnalysedType, TypeHandle,
        };
        use golem_wasm_rpc::{IntoValueAndType, Value, ValueAndType};
        use std::sync::Arc;
        use uuid::Uuid;

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

        pub(crate) fn configurable_metadata(
            function_name: &str,
            input_types: Vec<AnalysedType>,
            output: Option<AnalysedType>,
        ) -> Vec<ComponentDependency> {
            let analysed_function_parameters = input_types
                .into_iter()
                .enumerate()
                .map(|(index, typ)| AnalysedFunctionParameter {
                    name: format!("param{index}"),
                    typ,
                })
                .collect();

            let result = output.map(|typ| AnalysedFunctionResult { typ });

            let component_info = ComponentDependencyKey {
                component_name: "foo".to_string(),
                component_id: Uuid::new_v4(),
                root_package_name: None,
                root_package_version: None,
            };

            vec![ComponentDependency::new(
                component_info,
                vec![AnalysedExport::Function(AnalysedFunction {
                    name: function_name.to_string(),
                    parameters: analysed_function_parameters,
                    result,
                })],
            )]
        }

        pub(crate) fn get_metadata_with_resource_with_params() -> Vec<ComponentDependency> {
            get_metadata_with_resource(vec![AnalysedFunctionParameter {
                name: "user-id".to_string(),
                typ: str(),
            }])
        }

        pub(crate) fn get_metadata_with_resource_without_params() -> Vec<ComponentDependency> {
            get_metadata_with_resource(vec![])
        }

        pub(crate) fn get_metadata_with_multiple_interfaces() -> Vec<ComponentDependency> {
            // Exist in only amazon:shopping-cart/api1
            let analysed_function_in_api1 = AnalysedFunction {
                name: "foo".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "arg1".to_string(),
                    typ: str(),
                }],
                result: Some(AnalysedFunctionResult { typ: str() }),
            };

            let analysed_function_in_api1_number = AnalysedFunction {
                name: "foo-number".to_string(),
                parameters: vec![
                    AnalysedFunctionParameter {
                        name: "arg1".to_string(),
                        typ: u64(),
                    },
                    AnalysedFunctionParameter {
                        name: "arg2".to_string(),
                        typ: s32(),
                    },
                ],
                result: Some(AnalysedFunctionResult { typ: s32() }),
            };

            // Exist in both amazon:shopping-cart/api1 and amazon:shopping-cart/api2
            let analysed_function_in_api1_and_api2 = AnalysedFunction {
                name: "bar".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "arg1".to_string(),
                    typ: str(),
                }],
                result: Some(AnalysedFunctionResult { typ: str() }),
            };

            // Exist in only wasi:clocks/monotonic-clock
            let analysed_function_in_wasi = AnalysedFunction {
                name: "baz".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "arg1".to_string(),
                    typ: str(),
                }],
                result: Some(AnalysedFunctionResult { typ: str() }),
            };

            // Exist in wasi:clocks/monotonic-clock and amazon:shopping-cart/api1
            let analysed_function_in_wasi_and_api1 = AnalysedFunction {
                name: "qux".to_string(),
                parameters: vec![AnalysedFunctionParameter {
                    name: "arg1".to_string(),
                    typ: str(),
                }],
                result: Some(AnalysedFunctionResult { typ: str() }),
            };

            let analysed_export1 = AnalysedExport::Instance(AnalysedInstance {
                name: "amazon:shopping-cart/api1".to_string(),
                functions: vec![
                    analysed_function_in_api1,
                    analysed_function_in_api1_number,
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

            let component_info = ComponentDependencyKey {
                component_name: "foo".to_string(),
                component_id: Uuid::new_v4(),
                root_package_name: None,
                root_package_version: None,
            };

            vec![ComponentDependency::new(
                component_info,
                vec![analysed_export1, analysed_export2, analysed_export3],
            )]
        }

        fn get_metadata_with_resource(
            resource_constructor_params: Vec<AnalysedFunctionParameter>,
        ) -> Vec<ComponentDependency> {
            let instance = AnalysedExport::Instance(AnalysedInstance {
                name: "golem:it/api".to_string(),
                functions: vec![
                    AnalysedFunction {
                        name: "[constructor]cart".to_string(),
                        parameters: resource_constructor_params,
                        result: Some(AnalysedFunctionResult {
                            typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Owned),
                        }),
                    },
                    AnalysedFunction {
                        name: "[static]cart.create".to_string(),
                        parameters: vec![AnalysedFunctionParameter {
                            name: "item-name".to_string(),
                            typ: str(),
                        }],
                        result: Some(AnalysedFunctionResult {
                            typ: AnalysedType::Handle(TypeHandle {
                                name: Some("cart".to_string()),
                                owner: Some("golem:it/api".to_string()),
                                resource_id: AnalysedResourceId(0),
                                mode: AnalysedResourceMode::Owned,
                            }),
                        }),
                    },
                    AnalysedFunction {
                        name: "[static]cart.create-safe".to_string(),
                        parameters: vec![AnalysedFunctionParameter {
                            name: "item-name".to_string(),
                            typ: str(),
                        }],
                        result: Some(AnalysedFunctionResult {
                            typ: result(
                                AnalysedType::Handle(TypeHandle {
                                    name: Some("cart".to_string()),
                                    owner: Some("golem:it/api".to_string()),
                                    resource_id: AnalysedResourceId(0),
                                    mode: AnalysedResourceMode::Owned,
                                }),
                                str(),
                            ),
                        }),
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
                        result: None,
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
                        result: None,
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
                        result: None,
                    },
                    AnalysedFunction {
                        name: "[method]cart.checkout".to_string(),
                        parameters: vec![AnalysedFunctionParameter {
                            name: "self".to_string(),
                            typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                        }],
                        result: Some(AnalysedFunctionResult {
                            typ: variant(vec![
                                case("error", str()),
                                case("success", record(vec![field("order-id", str())])),
                            ]),
                        }),
                    },
                    AnalysedFunction {
                        name: "[method]cart.get-cart-contents".to_string(),
                        parameters: vec![AnalysedFunctionParameter {
                            name: "self".to_string(),
                            typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                        }],
                        result: Some(AnalysedFunctionResult {
                            typ: list(record(vec![
                                field("product-id", str()),
                                field("name", str()),
                                field("price", f32()),
                                field("quantity", u32()),
                            ])),
                        }),
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
                        result: None,
                    },
                    AnalysedFunction {
                        name: "[drop]cart".to_string(),
                        parameters: vec![AnalysedFunctionParameter {
                            name: "self".to_string(),
                            typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Owned),
                        }],
                        result: None,
                    },
                ],
            });

            let component_info = ComponentDependencyKey {
                component_name: "foo".to_string(),
                component_id: Uuid::new_v4(),
                root_package_name: None,
                root_package_version: None,
            };

            vec![ComponentDependency::new(component_info, vec![instance])]
        }

        pub(crate) fn get_value_and_type(
            analysed_type: &AnalysedType,
            wasm_wave_str: &str,
        ) -> ValueAndType {
            golem_wasm_rpc::parse_value_and_type(analysed_type, wasm_wave_str).unwrap()
        }

        pub(crate) fn interpreter_with_noop_function_invoke(
            input: Option<RibInput>,
        ) -> Interpreter {
            let invoke: Arc<dyn RibComponentFunctionInvoke + Send + Sync> =
                Arc::new(NoopRibFunctionInvoke);

            Interpreter {
                input: input.unwrap_or_default(),
                invoke,
                generate_worker_name: Arc::new(DefaultWorkerNameGenerator),
            }
        }

        // Interpreter which always returns a specific response
        pub(crate) fn interpreter_with_static_function_response(
            result_value: &ValueAndType,
            input: Option<RibInput>,
        ) -> Interpreter {
            let value = result_value.clone();

            let invoke = Arc::new(TestInvoke1 { value });

            Interpreter {
                input: input.unwrap_or_default(),
                invoke,
                generate_worker_name: Arc::new(DefaultWorkerNameGenerator),
            }
        }

        // The interpreter that always returns a record value consisting of function name, worker name etc
        // for every function calls in Rib.
        // Example : `my-instance.qux[amazon:shopping-cart]("bar")` will return a record
        // that contains the actual worker-name of my-instance, the function name `qux` and arguments
        // It helps ensures that interpreter invokes the function at the expected worker.
        pub(crate) fn interpreter_with_resource_function_invoke_impl(
            rib_input: Option<RibInput>,
        ) -> Interpreter {
            let invoke: Arc<dyn RibComponentFunctionInvoke + Send + Sync> =
                Arc::new(ResourceFunctionsInvoke);

            Interpreter {
                input: rib_input.unwrap_or_default(),
                invoke,
                generate_worker_name: Arc::new(DefaultWorkerNameGenerator),
            }
        }

        // A simple interpreter that returns response based on the function
        pub(crate) fn interpreter_for_global_functions(input: Option<RibInput>) -> Interpreter {
            let invoke = Arc::new(TestInvoke3);

            Interpreter {
                input: input.unwrap_or_default(),
                invoke,
                generate_worker_name: Arc::new(DefaultWorkerNameGenerator),
            }
        }

        struct TestInvoke1 {
            value: ValueAndType,
        }

        #[async_trait]
        impl RibComponentFunctionInvoke for TestInvoke1 {
            async fn invoke(
                &self,
                _component_dependency_key: ComponentDependencyKey,
                _instruction_id: &InstructionId,
                _worker_name: Option<EvaluatedWorkerName>,
                _fqn: EvaluatedFqFn,
                _args: EvaluatedFnArgs,
                _return_type: Option<AnalysedType>,
            ) -> RibFunctionInvokeResult {
                let value = self.value.clone();
                Ok(Some(value))
            }
        }

        struct PassThroughFunctionInvoke;

        #[async_trait]
        impl RibComponentFunctionInvoke for PassThroughFunctionInvoke {
            async fn invoke(
                &self,
                _component_dependency_key: ComponentDependencyKey,
                _instruction_id: &InstructionId,
                worker_name: Option<EvaluatedWorkerName>,
                function_name: EvaluatedFqFn,
                args: EvaluatedFnArgs,
                _return_type: Option<AnalysedType>,
            ) -> RibFunctionInvokeResult {
                let analysed_type = record(vec![
                    field("worker-name", str()),
                    field("function-name", str()),
                    field("args0", u64()),
                    field("args1", u32()),
                ]);

                let worker_name = Value::String(worker_name.map(|x| x.0).unwrap_or_default());
                let function_name = Value::String(function_name.0);
                let args0 = args.0[0].value.clone();
                let args1 = args.0[1].value.clone();

                let value = Value::Record(vec![worker_name, function_name, args0, args1]);

                Ok(Some(ValueAndType::new(value, analysed_type)))
            }
        }

        struct ResourceFunctionsInvoke;

        #[async_trait]
        impl RibComponentFunctionInvoke for ResourceFunctionsInvoke {
            async fn invoke(
                &self,
                _component_dependency_key: ComponentDependencyKey,
                _instruction_id: &InstructionId,
                worker_name: Option<EvaluatedWorkerName>,
                function_name: EvaluatedFqFn,
                args: EvaluatedFnArgs,
                _return_type: Option<AnalysedType>,
            ) -> RibFunctionInvokeResult {
                match function_name.0.as_str() {
                    "golem:it/api.{cart.new}" => {
                        let worker_name = worker_name.map(|x| x.0).unwrap_or_default();

                        let uri = format!(
                            "urn:worker:99738bab-a3bf-4a12-8830-b6fd783d1ef2/{worker_name}"
                        );
                        Ok(ValueAndType::new(
                            Value::Handle {
                                uri,
                                resource_id: 0,
                            },
                            handle(AnalysedResourceId(0), AnalysedResourceMode::Owned),
                        )
                        .into())
                    }

                    "golem:it/api.{cart.checkout}" => {
                        let result_type = variant(vec![
                            case("error", str()),
                            case("success", record(vec![field("order-id", str())])),
                        ]);

                        let result_value = get_value_and_type(
                            &result_type,
                            r#"
                            success({order-id: "foo"})
                            "#,
                        );

                        Ok(Some(result_value))
                    }

                    "golem:it/api.{cart.add-item}" => Ok(None),

                    "golem:it/api.{cart.update-item-quantity}" => Ok(None),

                    "golem:it/api.{cart.remove-item}" => Ok(None),

                    "golem:it/api.{cart.drop}" => Ok(None),

                    "golem:it/api.{cart.get-cart-contents}" => {
                        let typ = list(record(vec![
                            field("product-id", str()),
                            field("name", str()),
                            field("price", f32()),
                            field("quantity", u32()),
                        ]));

                        let value = Value::Record(vec![
                            Value::String("foo".to_string()),
                            Value::String("bar".to_string()),
                            Value::F32(10.0),
                            Value::U32(2),
                        ]);

                        Ok(Some(ValueAndType::new(Value::List(vec![value]), typ)))
                    }

                    "golem:it/api.{[static]cart.create}" => {
                        let uri = format!(
                            "urn:worker:99738bab-a3bf-4a12-8830-b6fd783d1ef2/{}",
                            worker_name.map(|x| x.0).unwrap_or_default()
                        );

                        let value = Value::Handle {
                            uri,
                            resource_id: 0,
                        };

                        Ok(Some(ValueAndType::new(
                            value,
                            handle(AnalysedResourceId(0), AnalysedResourceMode::Owned),
                        )))
                    }

                    "golem:it/api.{[static]cart.create-safe}" => {
                        let uri = format!(
                            "urn:worker:99738bab-a3bf-4a12-8830-b6fd783d1ef2/{}",
                            worker_name.map(|x| x.0).unwrap_or_default()
                        );

                        let resource = Value::Handle {
                            uri,
                            resource_id: 0,
                        };

                        let value = Value::Result(Ok(Some(Box::new(resource))));

                        Ok(Some(ValueAndType::new(
                            value,
                            result(
                                handle(AnalysedResourceId(0), AnalysedResourceMode::Owned),
                                str(),
                            ),
                        )))
                    }

                    "golem:it/api.{cart.pass-through}" => {
                        let worker_name = worker_name.map(|x| x.0);
                        let function_args = args.0[1..].to_vec();

                        let mut arg_types = vec![];

                        for (index, value_and_type) in function_args.iter().enumerate() {
                            let name = format!("args{index}");
                            let value = value_and_type.typ.clone();
                            arg_types.push(field(name.as_str(), value));
                        }

                        let function_name = function_name.0.into_value_and_type();

                        let mut analysed_type_pairs = vec![];
                        analysed_type_pairs.push(field("worker-name", option(str())));
                        analysed_type_pairs.push(field("function-name", str()));
                        analysed_type_pairs.extend(arg_types);

                        let mut values = vec![];

                        values.push(Value::Option(
                            worker_name.map(|x| Box::new(Value::String(x))),
                        ));
                        values.push(function_name.value);

                        for arg_value in function_args {
                            values.push(arg_value.value);
                        }

                        let value_and_type =
                            ValueAndType::new(Value::Record(values), record(analysed_type_pairs));

                        Ok(Some(value_and_type))
                    }

                    _ => Err(format!("unexpected function name: {}", function_name.0).into()),
                }
            }
        }

        struct MultiplePackageFunctionInvoke;

        #[async_trait]
        impl RibComponentFunctionInvoke for MultiplePackageFunctionInvoke {
            async fn invoke(
                &self,
                _component_dependency_key: ComponentDependencyKey,
                _instruction_id: &InstructionId,
                _worker_name: Option<EvaluatedWorkerName>,
                function_name: EvaluatedFqFn,
                _args: EvaluatedFnArgs,
                _return_type: Option<AnalysedType>,
            ) -> RibFunctionInvokeResult {
                match function_name.0.as_str() {
                    "amazon:shopping-cart/api1.{foo}" => {
                        let result_value =
                            ValueAndType::new(Value::String("foo".to_string()), str());

                        Ok(Some(result_value))
                    }

                    "amazon:shopping-cart/api1.{foo-number}" => {
                        let result_value = ValueAndType::new(Value::S32(1), s32());

                        Ok(Some(result_value))
                    }

                    "amazon:shopping-cart/api1.{bar}" => {
                        let result_value =
                            ValueAndType::new(Value::String("api1-bar".to_string()), str());

                        Ok(Some(result_value))
                    }

                    "amazon:shopping-cart/api1.{qux}" => {
                        let result_value =
                            ValueAndType::new(Value::String("qux".to_string()), str());

                        Ok(Some(result_value))
                    }

                    "amazon:shopping-cart/api2.{bar}" => {
                        let result_value =
                            ValueAndType::new(Value::String("api2-bar".to_string()), str());

                        Ok(Some(result_value))
                    }

                    "wasi:clocks/monotonic-clock.{baz}" => {
                        let result_value =
                            ValueAndType::new(Value::String("clock-baz".to_string()), str());

                        Ok(Some(result_value))
                    }

                    "wasi:clocks/monotonic-clock.{qux}" => {
                        let result_value =
                            ValueAndType::new(Value::String("clock-qux".to_string()), str());

                        Ok(Some(result_value))
                    }

                    _ => Err(format!("unexpected function name: {}", function_name.0).into()),
                }
            }
        }

        pub(crate) struct StaticWorkerNameGenerator;

        impl GenerateWorkerName for StaticWorkerNameGenerator {
            fn generate_worker_name(&self) -> String {
                "test-worker".to_string()
            }
        }

        pub(crate) struct RibTestDeps {
            pub(crate) component_dependencies: Vec<ComponentDependency>,
            pub(crate) interpreter: Interpreter,
        }

        impl RibTestDeps {
            pub(crate) fn test_deps_with_global_functions() -> RibTestDeps {
                let component_dependencies = get_component_dependency_with_global_functions();
                let interpreter = interpreter_for_global_functions(None);

                RibTestDeps {
                    component_dependencies,
                    interpreter,
                }
            }

            pub(crate) fn test_deps_with_resource_functions(
                rib_input: Option<RibInput>,
            ) -> RibTestDeps {
                let component_dependencies = get_metadata_with_resource_without_params();
                let interpreter = interpreter_with_resource_function_invoke_impl(rib_input);

                RibTestDeps {
                    component_dependencies,
                    interpreter,
                }
            }

            pub(crate) fn test_deps_with_indexed_resource_functions(
                rib_input: Option<RibInput>,
            ) -> RibTestDeps {
                let component_dependencies = get_metadata_with_resource_with_params();
                let interpreter = interpreter_with_resource_function_invoke_impl(rib_input);

                RibTestDeps {
                    component_dependencies,
                    interpreter,
                }
            }

            // A pass through function simply pass through the information embedded in a function call
            // such as function name, the worker name and the arguments used to invoke the call
            // allowing us to cross verify if the invoke is correct
            pub(crate) fn test_deps_for_pass_through_function() -> RibTestDeps {
                let exports = vec![AnalysedExport::Function(AnalysedFunction {
                    name: "pass-through".to_string(),
                    parameters: vec![
                        AnalysedFunctionParameter {
                            name: "item".to_string(),
                            typ: u64(),
                        },
                        AnalysedFunctionParameter {
                            name: "item".to_string(),
                            typ: u32(),
                        },
                    ],
                    result: Some(AnalysedFunctionResult {
                        typ: record(vec![
                            field("worker-name", option(str())),
                            field("function-name", str()),
                            field("args0", u64()),
                            field("args1", u32()),
                        ]),
                    }),
                })];

                let component_info = ComponentDependencyKey {
                    component_name: "foo".to_string(),
                    component_id: Uuid::new_v4(),
                    root_package_name: None,
                    root_package_version: None,
                };

                let exports = vec![ComponentDependency::new(component_info, exports)];

                let interpreter = Interpreter::new(
                    RibInput::default(),
                    Arc::new(PassThroughFunctionInvoke),
                    Arc::new(StaticWorkerNameGenerator),
                );

                RibTestDeps {
                    component_dependencies: exports,
                    interpreter,
                }
            }

            pub(crate) fn test_deps_with_multiple_interfaces(
                rib_input: Option<RibInput>,
            ) -> RibTestDeps {
                let component_dependencies = get_metadata_with_multiple_interfaces();
                let interpreter = Interpreter::new(
                    rib_input.unwrap_or_default(),
                    Arc::new(MultiplePackageFunctionInvoke),
                    Arc::new(StaticWorkerNameGenerator),
                );

                RibTestDeps {
                    component_dependencies,
                    interpreter,
                }
            }
        }

        fn get_component_dependency_with_global_functions() -> Vec<ComponentDependency> {
            let exports = vec![
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
                    result: Some(AnalysedFunctionResult { typ: u32() }),
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
                    result: Some(AnalysedFunctionResult { typ: u64() }),
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
                    result: Some(AnalysedFunctionResult {
                        typ: r#enum(&["x", "y", "z"]),
                    }),
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
                    result: Some(AnalysedFunctionResult {
                        typ: get_analysed_type_variant(),
                    }),
                }),
            ];

            let component_info = ComponentDependencyKey {
                component_name: "foo".to_string(),
                component_id: Uuid::new_v4(),
                root_package_name: None,
                root_package_version: None,
            };

            vec![ComponentDependency::new(component_info, exports)]
        }

        struct TestInvoke3;

        #[async_trait]
        impl RibComponentFunctionInvoke for TestInvoke3 {
            async fn invoke(
                &self,
                _component_dependency: ComponentDependencyKey,
                _instruction_id: &InstructionId,
                _worker_name: Option<EvaluatedWorkerName>,
                function_name: EvaluatedFqFn,
                args: EvaluatedFnArgs,
                _return_type: Option<AnalysedType>,
            ) -> RibFunctionInvokeResult {
                match function_name.0.as_str() {
                    "add-u32" => {
                        let args = args.0;
                        let arg1 = args[0].get_literal().and_then(|x| x.get_number()).unwrap();
                        let arg2 = args[1].get_literal().and_then(|x| x.get_number()).unwrap();
                        let result = (arg1 + arg2).unwrap();
                        let u32 = result.cast_to(&u32()).unwrap();

                        Ok(Some(u32))
                    }
                    "add-u64" => {
                        let args = args.0;
                        let arg1 = args[0].get_literal().and_then(|x| x.get_number()).unwrap();
                        let arg2 = args[1].get_literal().and_then(|x| x.get_number()).unwrap();
                        let result = (arg1 + arg2).unwrap();
                        let u64 = result.cast_to(&u64()).unwrap();
                        Ok(Some(u64))
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
                                    Ok(Some(result))
                                } else {
                                    Err(format!("Enums are not equal: {x} and {y}").into())
                                }
                            }
                            (v1, v2) => {
                                Err(format!("Invalid arguments for add-enum: {v1:?} and {v2:?}")
                                    .into())
                            }
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
                                    Ok(Some(result))
                                } else {
                                    Err(format!(
                                        "Variants are not equal: {case_idx1} and {case_idx2}"
                                    )
                                    .into())
                                }
                            }
                            (v1, v2) => Err(format!(
                                "Invalid arguments for add-variant: {v1:?} and {v2:?}"
                            )
                            .into()),
                        }
                    }
                    fun => Err(format!("unknown function {fun}").into()),
                }
            }
        }
    }
}
