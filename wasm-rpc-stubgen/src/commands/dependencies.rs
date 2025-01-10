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

use crate::wit_generate;
use crate::wit_generate::AddClientAsDepConfig;
use std::path::Path;

pub use crate::wit_generate::UpdateCargoToml;

pub fn add_stub_dependency(
    stub_wit_root: &Path,
    dest_wit_root: &Path,
    update_cargo_toml: UpdateCargoToml,
) -> anyhow::Result<()> {
    wit_generate::add_client_as_dependency_to_wit_dir(AddClientAsDepConfig {
        client_wit_root: stub_wit_root.to_path_buf(),
        dest_wit_root: dest_wit_root.to_path_buf(),
        update_cargo_toml,
    })
}
