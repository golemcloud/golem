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

/// Helper types for generated bridge crates
pub mod bridge;

use crate::model::MultipartField;
use golem_common::model::component::{ComponentCreation, ComponentUpdate};
use golem_common::model::plugin_registration::PluginRegistrationCreation;

include!(concat!(env!("OUT_DIR"), "/src/lib.rs"));

pub const LOCAL_WELL_KNOWN_TOKEN: &str = "5c832d93-ff85-4a8f-9803-513950fdfdb1";

#[cfg(test)]
test_r::enable!();

impl MultipartField for ComponentCreation {
    fn to_multipart_field(&self) -> String {
        serde_json::to_string(&self).unwrap()
    }

    fn mime_type(&self) -> &'static str {
        "application/json"
    }
}

impl MultipartField for ComponentUpdate {
    fn to_multipart_field(&self) -> String {
        serde_json::to_string(&self).unwrap()
    }

    fn mime_type(&self) -> &'static str {
        "application/json"
    }
}

impl MultipartField for PluginRegistrationCreation {
    fn to_multipart_field(&self) -> String {
        serde_json::to_string(&self).unwrap()
    }

    fn mime_type(&self) -> &'static str {
        "application/json"
    }
}
