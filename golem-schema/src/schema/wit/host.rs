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
            default: async | trappable,
        },
        exports: { default: async },
        require_store_data_send: true,
        anyhow: true,
        wasmtime_crate: ::wasmtime,
    });
}
