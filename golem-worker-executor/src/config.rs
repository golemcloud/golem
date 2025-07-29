// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

#[cfg(test)]
mod tests {
    use std::env;
    use std::path::PathBuf;
    use test_r::test;

    use crate::services::golem_config::make_config_loader;

    #[test]
    pub fn config_is_loadable() {
        env::set_current_dir( PathBuf::from(env!("CARGO_MANIFEST_DIR")) ).expect("Failed to set current directory");

        make_config_loader()
            .load()
            .expect("Failed to load base config");
    }
}
