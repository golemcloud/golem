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

use golem_common::model::component::ComponentOwner;
use golem_common::model::component_constraint::FunctionConstraint;
use golem_common::model::component_constraint::FunctionConstraintCollection;
use golem_common::model::ComponentId;
use golem_component_service_base::model::ComponentConstraints;
use golem_wasm_ast::analysis::analysed_type::{f32, list, record, str, u32, u64};
use golem_wasm_ast::analysis::NameTypePair;
use rib::RegistryKey;

pub(crate) fn get_shopping_cart_worker_functions_constraint1() -> FunctionConstraintCollection {
    FunctionConstraintCollection {
        function_constraints: vec![FunctionConstraint {
            function_key: RegistryKey::FunctionNameWithInterface {
                interface_name: "golem:it/api".to_string(),
                function_name: "initialize-cart".to_string(),
            },
            parameter_types: vec![str()],
            return_types: vec![],
            usage_count: 1,
        }],
    }
}

pub(crate) fn get_shopping_cart_worker_functions_constraint2() -> FunctionConstraintCollection {
    FunctionConstraintCollection {
        function_constraints: vec![FunctionConstraint {
            function_key: RegistryKey::FunctionNameWithInterface {
                interface_name: "golem:it/api".to_string(),
                function_name: "get-cart-contents".to_string(),
            },
            usage_count: 1,
            parameter_types: vec![],

            return_types: vec![list(record(vec![
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
        }],
    }
}

pub(crate) fn get_shopping_cart_worker_functions_constraint_incompatible(
) -> FunctionConstraintCollection {
    FunctionConstraintCollection {
        function_constraints: vec![FunctionConstraint {
            function_key: RegistryKey::FunctionNameWithInterface {
                interface_name: "golem:it/api".to_string(),
                function_name: "initialize-cart".to_string(),
            },
            parameter_types: vec![u64()],
            return_types: vec![str()],
            usage_count: 1,
        }],
    }
}

pub(crate) fn get_random_worker_functions_constraint() -> FunctionConstraintCollection {
    FunctionConstraintCollection {
        function_constraints: vec![FunctionConstraint {
            usage_count: 1,
            function_key: RegistryKey::FunctionName("foo".to_string()),
            parameter_types: vec![],
            return_types: vec![list(record(vec![
                NameTypePair {
                    name: "bar".to_string(),
                    typ: str(),
                },
                NameTypePair {
                    name: "baz".to_string(),
                    typ: str(),
                },
            ]))],
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
