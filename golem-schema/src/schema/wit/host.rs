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

#[allow(unused)]
#[rustfmt::skip]
pub mod generated {
    wasmtime::component::bindgen!({
        path: "wit",
        world: "golem-schema",
        imports: {
            "golem:core/types.[static]schema-value-stream.unwrap": async | store | trappable,
            "golem:core/types.[static]schema-value-stream.wrap": async | store | trappable,
            default: async | store,
        },
        exports: { default: async },
        require_store_data_send: true,
        anyhow: true,
        wasmtime_crate: ::wasmtime,
        with: {
            "golem:core/types@2.0.0.quota-token": crate::schema::wit::QuotaTokenHandleRep,
            "golem:core/types@2.0.0.secret": crate::schema::wit::SecretHandleRep,
            "golem:core/types@2.0.0.schema-value-stream": crate::schema::SchemaValueStreamHandleRep,
        },
    });
}
