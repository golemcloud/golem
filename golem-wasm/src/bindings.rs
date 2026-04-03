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

use wit_bindgen::generate;

generate!({
    path: "wit",
    world: "wasm-rpc",
    generate_all,
    generate_unused_types: true,
    with: {
        "wasi:clocks/wall-clock@0.2.3": wasip2::clocks::wall_clock,
        "wasi:io/poll@0.2.3": wasip2::io::poll,
    }
});
