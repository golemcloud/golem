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

//! Runtime resource helpers used by the executor.
//!
//! `ResourceStore` and `ResourceTypeId` are host-side resource bookkeeping,
//! not value/type concerns, so they live here in `golem-common`.

use async_trait::async_trait;
use wasmtime::component::ResourceAny;

/// Resource-runtime URI identifying the owning agent of a resource handle.
///
/// This is host-side resource bookkeeping metadata, not a value/type model
/// concern.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Uri {
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, desert_rust::BinaryCodec)]
pub struct ResourceTypeId {
    /// Name of the WIT resource
    pub name: String,
    /// Owner of the resource, either an interface in a WIT package or a name of a world
    pub owner: String,
}

#[async_trait]
pub trait ResourceStore {
    fn self_uri(&self) -> Uri;
    async fn add(&mut self, resource: ResourceAny, name: ResourceTypeId) -> u64;
    async fn get(&mut self, resource_id: u64) -> Option<(ResourceTypeId, ResourceAny)>;
    async fn borrow(&self, resource_id: u64) -> Option<(ResourceTypeId, ResourceAny)>;
}
