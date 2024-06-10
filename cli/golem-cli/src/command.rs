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

use crate::cloud::model::{CloudComponentIdOrName, ProjectRef};
use crate::model::ComponentIdOrName;
use crate::oss::model::OssContext;

pub mod api_definition;
pub mod api_deployment;
pub mod component;
pub mod profile;
pub mod worker;

pub trait ComponentRefSplit<ProjectRef> {
    fn split(self) -> (ComponentIdOrName, Option<ProjectRef>);
}

impl ComponentRefSplit<OssContext> for ComponentIdOrName {
    fn split(self) -> (ComponentIdOrName, Option<OssContext>) {
        (self, None)
    }
}

impl ComponentRefSplit<ProjectRef> for CloudComponentIdOrName {
    fn split(self) -> (ComponentIdOrName, Option<ProjectRef>) {
        match self {
            CloudComponentIdOrName::Id(id) => (ComponentIdOrName::Id(id), None),
            CloudComponentIdOrName::Name(name, p) => (ComponentIdOrName::Name(name), Some(p)),
        }
    }
}
