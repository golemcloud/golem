// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
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

//! Runtime resource helpers used by the executor outside the value bridge.
//!
//! `ResourceStore` and `ResourceTypeId` are not value/type concerns; they were
//! historically tangled into the `golem-wasm` wasmtime value bridge. They live
//! here in `golem-common` so they survive the deletion of that bridge (and of
//! the `golem-wasm` crate).

use async_trait::async_trait;
use golem_wasm::Uri;
use wasmtime::component::ResourceAny;

#[derive(Debug, Clone, PartialEq, Eq, Hash, desert_rust::BinaryCodec)]
pub struct ResourceTypeId {
    /// Name of the WIT resource
    pub name: String,
    /// Owner of the resource, either an interface in a WIT package or a name of a world
    pub owner: String,
}

impl golem_wasm::IntoValue for ResourceTypeId {
    fn into_value(self) -> golem_wasm::Value {
        golem_wasm::Value::Record(vec![
            golem_wasm::Value::String(self.name),
            golem_wasm::Value::String(self.owner),
        ])
    }

    fn get_type() -> golem_wasm::analysis::AnalysedType {
        use golem_wasm::analysis::analysed_type::*;
        record(vec![field("name", str()), field("owner", str())])
            .named("resource-type-id")
            .owned("golem:api@1.5.0/oplog")
    }
}

impl golem_wasm::FromValue for ResourceTypeId {
    fn from_value(value: golem_wasm::Value) -> Result<Self, String> {
        match value {
            golem_wasm::Value::Record(fields) if fields.len() == 2 => {
                let mut iter = fields.into_iter();
                let name = <String as golem_wasm::FromValue>::from_value(iter.next().unwrap())?;
                let owner = <String as golem_wasm::FromValue>::from_value(iter.next().unwrap())?;
                Ok(ResourceTypeId { name, owner })
            }
            other => Err(format!(
                "Expected Record with 2 fields for ResourceTypeId, got {other:?}"
            )),
        }
    }
}

#[async_trait]
pub trait ResourceStore {
    fn self_uri(&self) -> Uri;
    async fn add(&mut self, resource: ResourceAny, name: ResourceTypeId) -> u64;
    async fn get(&mut self, resource_id: u64) -> Option<(ResourceTypeId, ResourceAny)>;
    async fn borrow(&self, resource_id: u64) -> Option<(ResourceTypeId, ResourceAny)>;
}
