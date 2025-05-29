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

// Golem OSS specific services (to be merged with the `cloud` module once everything else is merged).

#[derive(Clone)]
pub struct AdditionalDeps {}

impl Default for AdditionalDeps {
    fn default() -> Self {
        Self::new()
    }
}

impl AdditionalDeps {
    pub fn new() -> Self {
        Self {}
    }

    #[cfg(test)]
    #[allow(unused)]
    pub async fn mocked() -> Self {
        Self {}
    }
}
