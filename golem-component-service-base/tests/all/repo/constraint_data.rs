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

use golem_common::model::component::ComponentOwner;
use golem_common::model::component_constraint::FunctionUsageConstraint;
use golem_common::model::component_constraint::{FunctionConstraints, FunctionSignature};
use golem_common::model::ComponentId;
use golem_component_service_base::model::ComponentConstraints;
use golem_wasm_ast::analysis::analysed_type::{f32, list, record, str, u32, u64};
use golem_wasm_ast::analysis::NameTypePair;
use rib::RegistryKey;

pub(crate) fn get_shopping_cart_worker_functions_constraint1() -> FunctionConstraints {
    FunctionConstraints {
        constraints: vec![FunctionUsageConstraint {
            function_signature: FunctionSignature::new(
                RegistryKey::FunctionNameWithInterface {
                    interface_name: "golem:it/api".to_string(),
                    function_name: "initialize-cart".to_string(),
                },
                vec![str()],
                vec![],
            ),
            usage_count: 1,
        }],
    }
}

pub(crate) fn get_shopping_cart_worker_functions_constraint2() -> FunctionConstraints {
    FunctionConstraints {
        constraints: vec![FunctionUsageConstraint {
            function_signature: FunctionSignature::new(
                RegistryKey::FunctionNameWithInterface {
                    interface_name: "golem:it/api".to_string(),
                    function_name: "get-cart-contents".to_string(),
                },
                vec![],
                vec![list(record(vec![
                    NameTypePair {
                        name: "product-id".to_string(),
                        typ: str(),
                    },
                    NameTypePair {
                        name: "name".to_string(),
                        typ: str(),
                    },
                    NameTypePair {
                        name: "price".to_string(),
                        typ: f32(),
                    },
                    NameTypePair {
                        name: "quantity".to_string(),
                        typ: u32(),
                    },
                ]))],
            ),
            usage_count: 1,
        }],
    }
}

pub(crate) fn get_shopping_cart_worker_functions_constraint_incompatible() -> FunctionConstraints {
    FunctionConstraints {
        constraints: vec![FunctionUsageConstraint {
            function_signature: FunctionSignature::new(
                RegistryKey::FunctionNameWithInterface {
                    interface_name: "golem:it/api".to_string(),
                    function_name: "initialize-cart".to_string(),
                },
                vec![u64()],
                vec![str()],
            ),
            usage_count: 1,
        }],
    }
}

pub(crate) fn get_random_worker_functions_constraint() -> FunctionConstraints {
    FunctionConstraints {
        constraints: vec![FunctionUsageConstraint {
            function_signature: FunctionSignature::new(
                RegistryKey::FunctionName("foo".to_string()),
                vec![],
                vec![list(record(vec![
                    NameTypePair {
                        name: "bar".to_string(),
                        typ: str(),
                    },
                    NameTypePair {
                        name: "baz".to_string(),
                        typ: str(),
                    },
                ]))],
            ),
            usage_count: 1,
        }],
    }
}

pub(crate) fn get_shopping_cart_component_constraint1<Owner: ComponentOwner>(
    owner: &Owner,
    component_id: &ComponentId,
) -> ComponentConstraints<Owner> {
    ComponentConstraints {
        owner: owner.clone(),
        component_id: component_id.clone(),
        constraints: get_shopping_cart_worker_functions_constraint1(),
    }
}

pub(crate) fn get_shopping_cart_component_constraint2<Owner: ComponentOwner>(
    owner: &Owner,
    component_id: &ComponentId,
) -> ComponentConstraints<Owner> {
    ComponentConstraints {
        owner: owner.clone(),
        component_id: component_id.clone(),
        constraints: get_shopping_cart_worker_functions_constraint2(),
    }
}

pub(crate) fn get_random_constraint<Owner: ComponentOwner>(
    owner: &Owner,
    component_id: &ComponentId,
) -> ComponentConstraints<Owner> {
    ComponentConstraints {
        owner: owner.clone(),
        component_id: component_id.clone(),
        constraints: get_random_worker_functions_constraint(),
    }
}

pub(crate) fn get_incompatible_constraint<Owner: ComponentOwner>(
    owner: &Owner,
    component_id: &ComponentId,
) -> ComponentConstraints<Owner> {
    ComponentConstraints {
        owner: owner.clone(),
        component_id: component_id.clone(),
        constraints: get_shopping_cart_worker_functions_constraint_incompatible(),
    }
}
