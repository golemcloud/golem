// Copyright 2024-2026 Golem Cloud
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
    DurableExecutionState, DurableFunctionType as RawDurableFunctionType,
    LazyInitializedPollable as RawLazyInitializedPollable,
    OplogEntryVersion as RawOplogEntryVersion, OplogIndex, PersistedDurableFunctionInvocation,
    PersistenceLevel, begin_durable_function, current_durable_execution_state,
    end_durable_function, observe_function_call, persist_durable_function_invocation,
    read_persisted_durable_function_invocation,
};
use crate::schema::{FromSchema, IntoSchema, IntoTypedSchemaValue, TypedSchemaValue};
use std::fmt::{Debug, Display};
use std::marker::PhantomData;

#[derive(Clone, Copy, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
pub enum DurableFunctionType {
    ReadLocal,
    WriteLocal,
    ReadRemote,
    WriteRemote,
    WriteRemoteBatched(Option<OplogIndex>),
    WriteRemoteTransaction(Option<OplogIndex>),
}

impl From<DurableFunctionType> for RawDurableFunctionType {
    fn from(value: DurableFunctionType) -> Self {
        match value {
            DurableFunctionType::ReadLocal => Self::ReadLocal,
            DurableFunctionType::WriteLocal => Self::WriteLocal,
            DurableFunctionType::ReadRemote => Self::ReadRemote,
            DurableFunctionType::WriteRemote => Self::WriteRemote,
            DurableFunctionType::WriteRemoteBatched(index) => Self::WriteRemoteBatched(index),
            DurableFunctionType::WriteRemoteTransaction(index) => {
                Self::WriteRemoteTransaction(index)
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, IntoSchema, FromSchema)]
pub enum OplogEntryVersion {
    V1,
    V2,
}

impl From<RawOplogEntryVersion> for OplogEntryVersion {
    fn from(value: RawOplogEntryVersion) -> Self {
        match value {
            RawOplogEntryVersion::V1 => Self::V1,
            RawOplogEntryVersion::V2 => Self::V2,
        }
    }
}

pub struct LazyInitializedPollable {
    raw: RawLazyInitializedPollable,
}

impl LazyInitializedPollable {
    pub fn new() -> Self {
        Self {
            raw: RawLazyInitializedPollable::new(),
        }
    }

    pub fn set(&self, pollable: wasip2::io::poll::Pollable) {
        self.raw.set(pollable)
    }

    pub fn subscribe(&self) -> wasip2::io::poll::Pollable {
        self.raw.subscribe()
    }
}

impl Default for LazyInitializedPollable {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Durability<SOk, SErr> {
    interface: &'static str,
    function: &'static str,
    function_type: RawDurableFunctionType,
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
        let function_type = RawDurableFunctionType::from(function_type);

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
        SIn: Debug + IntoSchema,
        SErr: Debug + for<'a> From<&'a Err>,
        SOk: Debug + From<Ok>,
        Result<SOk, SErr>: IntoSchema,
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
        SIn: Debug + IntoSchema,
        SOk: Debug + From<Ok>,
        SErr: Debug,
        Result<SOk, SErr>: IntoSchema,
    {
        let serializable_result: Result<SOk, SErr> = Ok(result.clone().into());

        self.persist_serializable(input, serializable_result);
        result
    }

    pub fn persist_serializable<SIn>(&self, input: SIn, result: Result<SOk, SErr>)
    where
        SIn: Debug + IntoSchema,
        Result<SOk, SErr>: IntoSchema,
    {
        let function_name = self.function_name();
        if !matches!(
            self.durable_execution_state.persistence_level,
            PersistenceLevel::PersistNothing
        ) {
            let request = input
                .into_typed_schema_value()
                .unwrap_or_else(|err| panic!("Failed serializing durable function input: {err}"));
            let response = result
                .into_typed_schema_value()
                .unwrap_or_else(|err| panic!("Failed serializing durable function result: {err}"));
            let request = crate::encode_typed_schema_value(&request)
                .unwrap_or_else(|err| panic!("Failed encoding durable function input: {err}"));
            let response = crate::encode_typed_schema_value(&response)
                .unwrap_or_else(|err| panic!("Failed encoding durable function result: {err}"));

            persist_durable_function_invocation(
                &function_name,
                &request,
                &response,
                self.function_type,
            );
            end_durable_function(self.function_type, self.begin_index, self.forced_commit);
        }
    }

    pub fn replay_raw(&self) -> (TypedSchemaValue, OplogEntryVersion) {
        let oplog_entry = read_persisted_durable_function_invocation();

        let function_name = self.function_name();
        Self::validate_oplog_entry(&oplog_entry, &function_name);

        end_durable_function(self.function_type, self.begin_index, false);

        let response = crate::decode_typed_schema_value(&oplog_entry.response)
            .unwrap_or_else(|err| panic!("Failed decoding durable function response: {err}"));
        (response, oplog_entry.entry_version.into())
    }

    pub fn replay_serializable(&self) -> Result<SOk, SErr>
    where
        SOk: FromSchema,
        SErr: FromSchema,
    {
        let (typed_schema_value, _) = self.replay_raw();
        let result: Result<SOk, SErr> = FromSchema::from_value(typed_schema_value.value())
            .unwrap_or_else(|err| panic!("Unexpected HostCall payload: {err}"));
        result
    }

    pub fn replay<Ok, Err>(&self) -> Result<Ok, Err>
    where
        Ok: From<SOk>,
        Err: From<SErr>,
        SErr: Debug + FromSchema,
        SOk: Debug + FromSchema,
    {
        Self::replay_serializable(self)
            .map(|sok| sok.into())
            .map_err(|serr| serr.into())
    }

    pub fn replay_infallible<Ok>(&self) -> Ok
    where
        Ok: From<SOk>,
        SOk: FromSchema,
        SErr: FromSchema + Display,
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
        oplog_entry: &PersistedDurableFunctionInvocation,
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
    use crate::durability::DurableFunctionType;
    use crate::{FromSchema, IntoSchema};
    use std::io::Error;

    // This is not an actual runnable test - with no host implementation - but verifies through
    // an example that the Durability API is usable.
    #[allow(dead_code)]
    fn durability_interface_test() {
        #[derive(Debug, IntoSchema, FromSchema)]
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
