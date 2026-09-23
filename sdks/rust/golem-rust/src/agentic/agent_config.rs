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
use crate::schema::wit::direct::{self, FromWire, WireSchema};
use crate::schema::{IntoSchema, SchemaGraph, SchemaType, SecretSpec};
use crate::secrets::GuestSecretHandle;
use std::marker::PhantomData;

pub use crate::bindings::golem::secrets::types::SecretError;
pub use crate::golem_agentic::golem::agent::host::ConfigValueError;

#[derive(Debug)]
pub enum SecretAccessError {
    Config(ConfigValueError),
    Reveal(SecretError),
}

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

    pub fn get(&self) -> Result<T, ConfigValueError>
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

    pub fn wire_config_entries(
        builder: &mut crate::schema::wit::direct::WireSchemaBuilder,
    ) -> Vec<crate::golem_agentic::golem::agent::common::AgentConfigDeclaration>
    where
        T: ConfigSchema,
    {
        T::describe_wire_config(&[], builder)
    }
}

pub trait ConfigSchema: Sized {
    /// Version of this type that can be used to get a remote
    /// agent instance using rpc.
    type RpcType: IntoRpcConfigParam;

    fn describe_config(path: &[String]) -> Vec<ExtendedAgentConfigDeclaration>;
    fn describe_wire_config(
        path: &[String],
        builder: &mut crate::schema::wit::direct::WireSchemaBuilder,
    ) -> Vec<crate::golem_agentic::golem::agent::common::AgentConfigDeclaration>;
    fn load(path: &[String]) -> Result<Self, ConfigValueError>;
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

    pub fn get(&self) -> Result<T, SecretAccessError>
    where
        T: FromWire + WireSchema,
    {
        let handle = self.handle().map_err(SecretAccessError::Config)?;
        let value = handle
            .with_handle(|handle| {
                crate::bindings::golem::secrets::reveal::reveal(handle, &direct::schema::<T>())
            })
            .expect("secret handle has already been transferred")
            .map_err(SecretAccessError::Reveal)?;
        Ok(direct::decode(value).expect("failed deserializing secret value"))
    }

    pub fn handle(&self) -> Result<GuestSecretHandle, ConfigValueError>
    where
        T: WireSchema,
    {
        let mut builder = direct::WireSchemaBuilder::default();
        let inner = T::append_schema(&mut builder);
        let root = builder.push(crate::schema::wit::wire::SchemaTypeBody::SecretType(
            crate::schema::wit::wire::SecretSpec {
                inner,
                category: None,
            },
        ));
        let value = get_config_value(&self.path, &builder.finish(root))?;
        Ok(direct::decode(value).expect("failed deserializing secret handle"))
    }
}

pub fn secret_schema_graph<T>() -> Result<SchemaGraph, crate::schema::validation::SchemaError>
where
    T: IntoSchema,
{
    crate::schema::try_into_schema_graph::<T>().map(secret_schema_graph_from_inner)
}

fn secret_schema_graph_from_inner(inner_graph: SchemaGraph) -> SchemaGraph {
    SchemaGraph {
        defs: inner_graph.defs,
        root: SchemaType::secret(SecretSpec {
            inner: Box::new(inner_graph.root),
            category: None,
        }),
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
