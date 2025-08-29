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

use crate::model::MultipartField;
use golem_common::model::component::{NewComponentData, UpdatedComponentData};
use golem_common::model::plugin_registration::NewPluginRegistrationData;

include!(concat!(env!("OUT_DIR"), "/src/lib.rs"));

#[cfg(test)]
test_r::enable!();

impl MultipartField for NewComponentData {
    fn to_multipart_field(&self) -> String {
        serde_json::to_string(&self).unwrap()
    }

    fn mime_type(&self) -> &'static str {
        "application/json"
    }
}

impl MultipartField for UpdatedComponentData {
    fn to_multipart_field(&self) -> String {
        serde_json::to_string(&self).unwrap()
    }

    fn mime_type(&self) -> &'static str {
        "application/json"
    }
}

impl MultipartField for NewPluginRegistrationData {
    fn to_multipart_field(&self) -> String {
        serde_json::to_string(&self).unwrap()
    }

    fn mime_type(&self) -> &'static str {
        "application/json"
    }
}
