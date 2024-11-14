// Copyright 2024 Golem Cloud
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

mod component;
mod plugin;

pub use component::*;
use golem_common::model::{ComponentFilePathWithPermissionsList, ComponentType};
pub use plugin::*;
use poem_openapi::types::multipart::Upload;
use poem_openapi::Multipart;

#[derive(Multipart)]
#[oai(rename_all = "camelCase")]
pub struct UpdatePayload {
    component_type: Option<ComponentType>,
    component: Upload,
    files_permissions: Option<ComponentFilePathWithPermissionsList>,
    files: Option<Upload>,
}
