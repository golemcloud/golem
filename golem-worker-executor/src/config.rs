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

#[cfg(test)]
mod tests {
    use test_r::test;

    use golem_worker_executor_base::services::additional_config::{
        load_or_dump_config, make_additional_config_loader,
    };
    use golem_worker_executor_base::services::golem_config::make_config_loader;

    #[test]
    pub fn base_config_is_loadable() {
        make_config_loader()
            .load()
            .expect("Failed to load base config");
    }

    #[test]
    pub fn additional_config_is_loadable() {
        make_additional_config_loader()
            .load()
            .expect("Failed to load additional config");
    }

    #[test]
    pub fn merged_config_is_loadable() {
        load_or_dump_config().expect("Failed to load additional config");
    }
}
