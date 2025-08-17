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

use crate::bindings::golem::durability::durability::{
    begin_durable_function, current_durable_execution_state, end_durable_function,
    observe_function_call, persist_typed_durable_function_invocation,
    read_persisted_typed_durable_function_invocation, DurableExecutionState, DurableFunctionType,
    OplogEntryVersion, OplogIndex, PersistedTypedDurableFunctionInvocation, PersistenceLevel,
};
use crate::value_and_type::{FromValueAndType, IntoValueAndType};
use golem_wasm_rpc::golem_rpc_0_2_x::types::ValueAndType;
use std::fmt::{Debug, Display};
use std::marker::PhantomData;

pub struct Durability<SOk, SErr> {
    interface: &'static str,
    function: &'static str,
    function_type: DurableFunctionType,
    begin_index: OplogIndex,
    durable_execution_state: DurableExecutionState,
    forced_commit: bool,
    _sok: PhantomData<SOk>,
    _serr: PhantomData<SErr>,
}

impl<SOk, SErr> Durability<SOk, SErr> {
    pub fn new(
        interface: &'static str,
        function: &'static str,
        function_type: DurableFunctionType,
    ) -> Self {
        observe_function_call(interface, function);

        let begin_index = begin_durable_function(function_type);
        let durable_execution_state = current_durable_execution_state();

        Self {
            interface,
            function,
            function_type,
            begin_index,
            durable_execution_state,
            forced_commit: false,
            _sok: PhantomData,
            _serr: PhantomData,
        }
    }

    pub fn enabled_forced_commit(&mut self) {
        self.forced_commit = true;
    }

    pub fn is_live(&self) -> bool {
        self.durable_execution_state.is_live
            || matches!(
                self.durable_execution_state.persistence_level,
                PersistenceLevel::PersistNothing
            )
    }

    pub fn persist<SIn, Ok, Err>(&self, input: SIn, result: Result<Ok, Err>) -> Result<Ok, Err>
    where
        Ok: Clone,
        Err: From<SErr>,
        SIn: Debug + IntoValueAndType,
        SErr: Debug + for<'a> From<&'a Err>,
        SOk: Debug + From<Ok>,
        Result<SOk, SErr>: IntoValueAndType,
    {
        let serializable_result: Result<SOk, SErr> = result
            .as_ref()
            .map(|result| result.clone().into())
            .map_err(|err| err.into());

        self.persist_serializable(input, serializable_result);
        result
    }

    pub fn persist_infallible<SIn, Ok>(&self, input: SIn, result: Ok) -> Ok
    where
        Ok: Clone,
        SIn: Debug + IntoValueAndType,
        SOk: Debug + From<Ok>,
        SErr: Debug,
        Result<SOk, SErr>: IntoValueAndType,
    {
        let serializable_result: Result<SOk, SErr> = Ok(result.clone().into());

        self.persist_serializable(input, serializable_result);
        result
    }

    pub fn persist_serializable<SIn>(&self, input: SIn, result: Result<SOk, SErr>)
    where
        SIn: Debug + IntoValueAndType,
        Result<SOk, SErr>: IntoValueAndType,
    {
        let function_name = self.function_name();
        if !matches!(
            self.durable_execution_state.persistence_level,
            PersistenceLevel::PersistNothing
        ) {
            persist_typed_durable_function_invocation(
                &function_name,
                &input.into_value_and_type(),
                &result.into_value_and_type(),
                self.function_type,
            );
            end_durable_function(self.function_type, self.begin_index, self.forced_commit);
        }
    }

    pub fn replay_raw(&self) -> (ValueAndType, OplogEntryVersion) {
        let oplog_entry = read_persisted_typed_durable_function_invocation();

        let function_name = self.function_name();
        Self::validate_oplog_entry(&oplog_entry, &function_name);

        end_durable_function(self.function_type, self.begin_index, false);

        (oplog_entry.response, oplog_entry.entry_version)
    }

    pub fn replay_serializable(&self) -> Result<SOk, SErr>
    where
        SOk: FromValueAndType,
        SErr: FromValueAndType,
    {
        let (value_and_type, _) = self.replay_raw();
        let result: Result<SOk, SErr> = FromValueAndType::from_value_and_type(value_and_type)
            .unwrap_or_else(|err| panic!("Unexpected ImportedFunctionInvoked payload: {err}"));
        result
    }

    pub fn replay<Ok, Err>(&self) -> Result<Ok, Err>
    where
        Ok: From<SOk>,
        Err: From<SErr>,
        SErr: Debug + FromValueAndType,
        SOk: Debug + FromValueAndType,
    {
        Self::replay_serializable(self)
            .map(|sok| sok.into())
            .map_err(|serr| serr.into())
    }

    pub fn replay_infallible<Ok>(&self) -> Ok
    where
        Ok: From<SOk>,
        SOk: FromValueAndType,
        SErr: FromValueAndType + Display,
    {
        let result: Result<SOk, SErr> = self.replay_serializable();
        result.map(|sok| sok.into()).unwrap_or_else(|err| {
            panic!(
                "Function {} previously failed with {}",
                self.function_name(),
                err
            )
        })
    }

    fn function_name(&self) -> String {
        if self.interface.is_empty() {
            // For backward compatibility - some of the recorded function names were not following the pattern
            self.function.to_string()
        } else {
            format!("{}::{}", self.interface, self.function)
        }
    }

    fn validate_oplog_entry(
        oplog_entry: &PersistedTypedDurableFunctionInvocation,
        expected_function_name: &str,
    ) {
        if oplog_entry.function_name != expected_function_name {
            panic!(
                "Unexpected imported function call entry in oplog: expected {}, got {}",
                expected_function_name, oplog_entry.function_name
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::bindings::golem::durability::durability::DurableFunctionType;
    use crate::value_and_type::type_builder::TypeNodeBuilder;
    use crate::value_and_type::{FromValueAndType, IntoValue};
    use golem_wasm_rpc::{NodeBuilder, WitValueExtractor};
    use std::io::Error;

    // This is not an actual runnable test - with no host implementation - but verifies through
    // an example that the Durability API is usable.
    #[allow(dead_code)]
    fn durability_interface_test() {
        #[derive(Debug)]
        enum CustomError {
            Error1,
            Error2,
        }

        impl From<&std::io::Error> for CustomError {
            fn from(_value: &Error) -> Self {
                Self::Error1
            }
        }

        impl From<CustomError> for std::io::Error {
            fn from(value: CustomError) -> Self {
                Error::other(format!("{value:?}"))
            }
        }

        impl IntoValue for CustomError {
            fn add_to_builder<T: NodeBuilder>(self, builder: T) -> T::Result {
                match self {
                    CustomError::Error1 => builder.enum_value(0),
                    CustomError::Error2 => builder.enum_value(1),
                }
            }

            fn add_to_type_builder<T: TypeNodeBuilder>(builder: T) -> T::Result {
                builder.r#enum(Some("CustomError".to_string()), None, &["Error1", "Error2"])
            }
        }

        impl FromValueAndType for CustomError {
            fn from_extractor<'a, 'b>(
                extractor: &'a impl WitValueExtractor<'a, 'b>,
            ) -> Result<Self, String> {
                match extractor.enum_value() {
                    Some(0) => Ok(CustomError::Error1),
                    Some(1) => Ok(CustomError::Error2),
                    _ => Err("Invalid enum value".to_string()),
                }
            }
        }

        fn durable_fn() -> Result<u64, std::io::Error> {
            let durability = super::Durability::<u64, CustomError>::new(
                "custom",
                "random-number-generator",
                DurableFunctionType::ReadLocal,
            );
            if durability.is_live() {
                let result = Ok(1234);
                durability.persist("input".to_string(), result)
            } else {
                durability.replay()
            }
        }
    }
}
