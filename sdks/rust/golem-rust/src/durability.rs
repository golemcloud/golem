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

use crate::bindings::golem::api::oplog::OplogIndex;
use crate::bindings::golem::durability::durability::{
    CustomDurableInvocation, DurableFunctionType as RawDurableFunctionType,
    LiveCustomDurableInvocation, OplogEntryVersion as RawOplogEntryVersion,
    PersistedDurableFunctionInvocation, begin_custom_durable_invocation, observe_function_call,
};
use crate::schema::{FromSchema, IntoSchema, IntoTypedSchemaValue};
use std::fmt::Display;
use std::future::Future;
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

#[must_use = "a durability invocation must be run to completion"]
pub struct Durability<SOk, SErr> {
    interface: &'static str,
    function: &'static str,
    invocation: CustomDurableInvocation,
    forced_commit: bool,
    _sok: PhantomData<SOk>,
    _serr: PhantomData<SErr>,
}

impl<SOk, SErr> Durability<SOk, SErr> {
    pub fn new<SIn>(
        interface: &'static str,
        function: &'static str,
        function_type: DurableFunctionType,
        input: &SIn,
    ) -> Self
    where
        SIn: Clone + IntoSchema,
    {
        observe_function_call(interface, function);
        let function_type = RawDurableFunctionType::from(function_type);
        let function_name = if interface.is_empty() {
            function.to_string()
        } else {
            format!("{interface}::{function}")
        };
        let request = input
            .clone()
            .into_typed_schema_value()
            .unwrap_or_else(|err| panic!("Failed serializing durable function input: {err}"));
        let request = crate::encode_typed_schema_value(&request)
            .unwrap_or_else(|err| panic!("Failed encoding durable function input: {err}"));
        let invocation = begin_custom_durable_invocation(&function_name, request, function_type);

        Self {
            interface,
            function,
            invocation,
            forced_commit: false,
            _sok: PhantomData,
            _serr: PhantomData,
        }
    }

    pub fn enable_forced_commit(&mut self) {
        self.forced_commit = true;
    }

    pub fn with_forced_commit(mut self) -> Self {
        self.forced_commit = true;
        self
    }

    pub fn run<Ok, Err, F>(self, body: F) -> Result<Ok, Err>
    where
        F: FnOnce() -> Result<Ok, Err>,
        Ok: Clone + From<SOk>,
        Err: From<SErr>,
        SErr: FromSchema + for<'a> From<&'a Err>,
        SOk: FromSchema + From<Ok>,
        Result<SOk, SErr>: IntoSchema,
    {
        match self.invocation {
            CustomDurableInvocation::Live(invocation) => {
                let result = body();
                let serializable_result = result
                    .as_ref()
                    .map(|result| result.clone().into())
                    .map_err(|err| err.into());
                Self::finish(invocation, serializable_result, self.forced_commit);
                result
            }
            CustomDurableInvocation::Replayed(ref invocation) => self
                .replay_serializable(invocation)
                .map(Into::into)
                .map_err(Into::into),
        }
    }

    pub async fn run_async<Ok, Err, F, Fut>(self, body: F) -> Result<Ok, Err>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Ok, Err>>,
        Ok: Clone + From<SOk>,
        Err: From<SErr>,
        SErr: FromSchema + for<'a> From<&'a Err>,
        SOk: FromSchema + From<Ok>,
        Result<SOk, SErr>: IntoSchema,
    {
        match self.invocation {
            CustomDurableInvocation::Live(invocation) => {
                let result = body().await;
                let serializable_result = result
                    .as_ref()
                    .map(|result| result.clone().into())
                    .map_err(|err| err.into());
                Self::finish(invocation, serializable_result, self.forced_commit);
                result
            }
            CustomDurableInvocation::Replayed(ref invocation) => self
                .replay_serializable(invocation)
                .map(Into::into)
                .map_err(Into::into),
        }
    }

    pub fn run_infallible<Ok, F>(self, body: F) -> Ok
    where
        F: FnOnce() -> Ok,
        Ok: Clone + From<SOk>,
        SOk: FromSchema + From<Ok>,
        SErr: FromSchema + Display,
        Result<SOk, SErr>: IntoSchema,
    {
        match self.invocation {
            CustomDurableInvocation::Live(invocation) => {
                let result = body();
                let serializable_result = Ok(result.clone().into());
                Self::finish(invocation, serializable_result, self.forced_commit);
                result
            }
            CustomDurableInvocation::Replayed(ref invocation) => {
                match self.replay_serializable(invocation) {
                    Ok(result) => result.into(),
                    Err(err) => panic!(
                        "Function {} previously failed with {}",
                        self.function_name(),
                        err
                    ),
                }
            }
        }
    }

    pub async fn run_infallible_async<Ok, F, Fut>(self, body: F) -> Ok
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Ok>,
        Ok: Clone + From<SOk>,
        SOk: FromSchema + From<Ok>,
        SErr: FromSchema + Display,
        Result<SOk, SErr>: IntoSchema,
    {
        match self.invocation {
            CustomDurableInvocation::Live(invocation) => {
                let result = body().await;
                let serializable_result = Ok(result.clone().into());
                Self::finish(invocation, serializable_result, self.forced_commit);
                result
            }
            CustomDurableInvocation::Replayed(ref invocation) => {
                match self.replay_serializable(invocation) {
                    Ok(result) => result.into(),
                    Err(err) => panic!(
                        "Function {} previously failed with {}",
                        self.function_name(),
                        err
                    ),
                }
            }
        }
    }

    fn finish(
        invocation: LiveCustomDurableInvocation,
        result: Result<SOk, SErr>,
        forced_commit: bool,
    ) where
        Result<SOk, SErr>: IntoSchema,
    {
        let response = result
            .into_typed_schema_value()
            .unwrap_or_else(|err| panic!("Failed serializing durable function result: {err}"));
        let response = crate::encode_typed_schema_value(&response)
            .unwrap_or_else(|err| panic!("Failed encoding durable function result: {err}"));
        LiveCustomDurableInvocation::finish(invocation, response, forced_commit);
    }

    fn replay_serializable(
        &self,
        invocation: &PersistedDurableFunctionInvocation,
    ) -> Result<SOk, SErr>
    where
        SOk: FromSchema,
        SErr: FromSchema,
    {
        Self::validate_oplog_entry(invocation, &self.function_name());
        let response = crate::decode_typed_schema_value(&invocation.response)
            .unwrap_or_else(|err| panic!("Failed decoding durable function response: {err}"));
        FromSchema::from_value(response.value())
            .unwrap_or_else(|err| panic!("Unexpected durable function payload: {err}"))
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
            let input = "input".to_string();
            super::Durability::<u64, CustomError>::new(
                "custom",
                "random-number-generator",
                DurableFunctionType::ReadLocal,
                &input,
            )
            .run(|| Ok(1234))
        }
    }
}
