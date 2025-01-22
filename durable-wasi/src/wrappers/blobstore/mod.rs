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

use bincode::{Decode, Encode};

mod blobstore;
mod container;
mod types;

#[derive(Debug, Clone, PartialEq, Eq, Encode, Decode)]
pub struct SerializableObjectMetadata {
    pub name: String,
    pub container: String,
    pub created_at: u64,
    pub size: u64,
}

impl From<crate::bindings::wasi::blobstore::types::ObjectMetadata> for SerializableObjectMetadata {
    fn from(value: crate::bindings::wasi::blobstore::types::ObjectMetadata) -> Self {
        SerializableObjectMetadata {
            name: value.name,
            container: value.container,
            created_at: value.created_at,
            size: value.size,
        }
    }
}

impl From<SerializableObjectMetadata>
    for crate::bindings::exports::wasi::blobstore::types::ObjectMetadata
{
    fn from(value: SerializableObjectMetadata) -> Self {
        crate::bindings::exports::wasi::blobstore::types::ObjectMetadata {
            name: value.name,
            container: value.container,
            created_at: value.created_at,
            size: value.size,
        }
    }
}
