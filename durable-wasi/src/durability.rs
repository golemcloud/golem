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

// NOTE: Similar to `Durability` in `golem-worker-executor-base`, but builds on the
// sync guest bindings to `golem:api/durability`.

use crate::bindings::golem::durability::durability::{
    begin_durable_function, current_durable_execution_state, end_durable_function,
    observe_function_call, persist_durable_function_invocation,
    read_persisted_durable_function_invocation, DurableExecutionState, DurableFunctionType,
    OplogEntryVersion, OplogIndex, PersistedDurableFunctionInvocation, PersistenceLevel,
};
use bincode::{Decode, Encode};
use bytes::Bytes;
use golem_common::serialization::{serialize, try_deserialize};
use std::fmt::{Debug, Display};
use std::marker::PhantomData;

pub struct Durability<SOk, SErr> {
    interface: &'static str,
    function: &'static str,
    function_type: DurableFunctionType,
    begin_index: OplogIndex,
    durable_execution_state: DurableExecutionState,
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
            _sok: PhantomData,
            _serr: PhantomData,
        }
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
        SIn: Debug + Encode,
        SErr: Debug + Encode + for<'a> From<&'a Err>,
        SOk: Debug + Encode + From<Ok>,
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
        SIn: Debug + Encode,
        SOk: Debug + Encode + From<Ok>,
        SErr: Debug + Encode,
    {
        let serializable_result: Result<SOk, SErr> = Ok(result.clone().into());

        self.persist_serializable(input, serializable_result);
        result
    }

    pub fn persist_serializable<SIn>(&self, input: SIn, result: Result<SOk, SErr>)
    where
        SIn: Debug + Encode,
        SOk: Debug + Encode,
        SErr: Debug + Encode,
    {
        let function_name = self.function_name();
        if !matches!(
            self.durable_execution_state.persistence_level,
            PersistenceLevel::PersistNothing
        ) {
            let serialized_input = serialize(&input).unwrap_or_else(|err| {
                panic!("failed to serialize input ({input:?}) for persisting durable function invocation: {err}")
            }).to_vec();
            let serialized_result = serialize(&result).unwrap_or_else(|err| {
                panic!("failed to serialize result ({result:?}) for persisting durable function invocation: {err}")
            }).to_vec();

            persist_durable_function_invocation(
                &function_name,
                &serialized_input,
                &serialized_result,
                self.function_type,
            );
            end_durable_function(self.function_type, self.begin_index);
        }
    }
    //
    // pub fn persist_typed_value<SIn>(&self, input: SIn, result: Result<SOk, SErr>)
    // where
    //     SIn: Debug + IntoValue,
    //     SOk: Debug + IntoValue,
    //     SErr: Debug + IntoValue,
    // {
    //     let function_name = self.function_name();
    //     if self.durable_execution_state.persistence_level != PersistenceLevel::PersistNothing {
    //         let input_value = input.into_value_and_type();
    //         let result_value = result.into_value_and_type();
    //
    //         ctx.persist_typed_durable_function_invocation(
    //             function_name.to_string(),
    //             input_value,
    //             result_value,
    //             self.function_type.clone(),
    //         );
    //         ctx.end_durable_function(&self.function_type, self.begin_index)?;
    //     }
    //     Ok(())
    // }

    pub fn replay_raw(&self) -> (Bytes, OplogEntryVersion) {
        let oplog_entry = read_persisted_durable_function_invocation();

        let function_name = self.function_name();
        Self::validate_oplog_entry(&oplog_entry, &function_name);

        end_durable_function(self.function_type, self.begin_index);

        (oplog_entry.response.into(), oplog_entry.entry_version)
    }

    pub fn replay_serializable(&self) -> Result<SOk, SErr>
    where
        SOk: Decode,
        SErr: Decode,
    {
        let (bytes, _) = self.replay_raw();
        let result: Result<SOk, SErr> = try_deserialize(&bytes)
            .unwrap_or_else(|err| panic!("Unexpected ImportedFunctionInvoked payload: {err}"))
            .expect("Payload is empty");
        result
    }

    pub fn replay<Ok, Err>(&self) -> Result<Ok, Err>
    where
        Ok: From<SOk>,
        Err: From<SErr>,
        SErr: Debug + Encode + Decode,
        SOk: Debug + Encode + Decode,
    {
        Self::replay_serializable(self)
            .map(|sok| sok.into())
            .map_err(|serr| serr.into())
    }

    pub fn replay_infallible<Ok>(&self) -> Ok
    where
        Ok: From<SOk>,
        SOk: Decode,
        SErr: Decode + Display,
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
