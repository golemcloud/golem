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

use crate::InterfaceName;
use golem_wasm_ast::analysis::AnalysedType;

#[derive(Clone, Debug)]
pub struct CustomInstanceSpec {
    pub instance_name: String,
    pub parameter_types_for_rib: Vec<AnalysedType>,
    pub parameter_types_for_instance_creation: Option<Vec<AnalysedType>>,
    pub interface_name: Option<InterfaceName>,
}

impl CustomInstanceSpec {
    // Constructs a new `CustomInstanceSpec`, which allows users to create instances other than the key word
    // `instance` that takes specific types of arguments and not just optional string
    //
    // Parameters:
    // - `instance_name`: The function name that can be used to create an instance.
    // - `parameter_types`: The types of parameters that the custom instance creation function takes.
    // - `interface_name`: If provided, it will associate this instance creation with the particular instance
    //                    and has to be part of the ComponentDependencies
    pub fn new(
        instance_name: String,
        parameter_types_for_rib: Vec<AnalysedType>,
        parameter_types_for_instance_creation: Option<Vec<AnalysedType>>,
        interface_name: Option<InterfaceName>,
    ) -> Self {
        CustomInstanceSpec {
            instance_name,
            parameter_types_for_rib,
            parameter_types_for_instance_creation,
            interface_name,
        }
    }

    pub fn parameter_types_for_instance_creation(&self) -> &Vec<AnalysedType> {
        match &self.parameter_types_for_instance_creation {
            Some(params) => params,
            None => &self.parameter_types_for_rib,
        }
    }
}
