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

use crate::agentic::ExtendedAgentConfigDeclaration;
use crate::golem_agentic::golem::agent::host::get_config_value;
use crate::schema::{FromSchema, IntoSchema};
use std::marker::PhantomData;

pub struct Config<T>(PhantomData<T>);

impl<T> Default for Config<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Config<T> {
    /// Creates a config handle.
    ///
    /// This exists primarily for SDK-generated code that materializes
    /// `#[agent_config]` constructor parameters. User code should receive
    /// `Config<T>` through constructor injection instead of constructing it
    /// manually, because agent config metadata is only registered from
    /// `#[agent_config]` constructor parameters.
    pub fn new() -> Self {
        Self(PhantomData)
    }

    pub fn get(&self) -> T
    where
        T: ConfigSchema,
    {
        T::load(&[])
    }

    pub fn config_entries() -> Vec<ExtendedAgentConfigDeclaration>
    where
        T: ConfigSchema,
    {
        T::describe_config(&[])
    }
}

pub trait ConfigSchema: Sized {
    /// Version of this type that can be used to get a remote
    /// agent instance using rpc.
    type RpcType: IntoRpcConfigParam;

    fn describe_config(path: &[String]) -> Vec<ExtendedAgentConfigDeclaration>;
    fn load(path: &[String]) -> Self;
}

#[doc(hidden)]
pub trait IntoRpcConfigParam: Sized {
    fn into_rpc_param(
        self,
        path: &[String],
    ) -> Vec<crate::golem_agentic::golem::agent::common::TypedAgentConfigValue>;
}

pub struct Secret<T> {
    path: Vec<String>,
    config_type: PhantomData<T>,
}

impl<T> Secret<T> {
    pub fn new(path: Vec<String>) -> Self {
        Self {
            path,
            config_type: PhantomData::<T>,
        }
    }

    pub fn get(&self) -> T
    where
        T: FromSchema + IntoSchema,
    {
        let graph = crate::schema::try_into_schema_graph::<T>()
            .expect("failed to build config schema graph");
        let value = get_config_value(
            &self.path,
            &crate::encode_schema_graph(&graph).expect("failed to encode config schema graph"),
        );
        let value =
            crate::decode_schema_value(value).expect("failed to decode config schema value");
        T::from_value(&value).expect("failed deserializing secret value")
    }
}

#[doc(hidden)]
pub trait InnerTypeHelper {
    type Type;
}

impl<T> InnerTypeHelper for Secret<T> {
    type Type = T;
}

impl<T> InnerTypeHelper for Config<T> {
    type Type = T;
}
