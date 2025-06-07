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
use rib::{FullyQualifiedFunctionName, FunctionName, InterfaceName, PackageName};

pub(crate) fn get_shopping_cart_worker_functions_constraint1() -> FunctionConstraints {
    FunctionConstraints {
        constraints: vec![FunctionUsageConstraint {
            function_signature: FunctionSignature::new(
                FunctionName::Function(FullyQualifiedFunctionName {
                    package_name: Some(PackageName {
                        namespace: "golem".to_string(),
                        package_name: "it".to_string(),
                        version: None,
                    }),
                    interface_name: Some(InterfaceName {
                        name: "api".to_string(),
                        version: None,
                    }),
                    function_name: "initialize-cart".to_string(),
                }),
                vec![str()],
                None,
            ),
            usage_count: 1,
        }],
    }
}

pub(crate) fn get_shopping_cart_worker_functions_constraint2() -> FunctionConstraints {
    FunctionConstraints {
        constraints: vec![FunctionUsageConstraint {
            function_signature: FunctionSignature::new(
                FunctionName::Function(FullyQualifiedFunctionName {
                    package_name: Some(PackageName {
                        namespace: "golem".to_string(),
                        package_name: "it".to_string(),
                        version: None,
                    }),
                    interface_name: Some(InterfaceName {
                        name: "api".to_string(),
                        version: None,
                    }),
                    function_name: "get-cart-contents".to_string(),
                }),
                vec![],
                Some(list(record(vec![
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
                ]))),
            ),
            usage_count: 1,
        }],
    }
}

pub(crate) fn get_shopping_cart_worker_functions_constraint_incompatible() -> FunctionConstraints {
    FunctionConstraints {
        constraints: vec![FunctionUsageConstraint {
            function_signature: FunctionSignature::new(
                FunctionName::Function(FullyQualifiedFunctionName {
                    package_name: Some(PackageName {
                        namespace: "golem".to_string(),
                        package_name: "it".to_string(),
                        version: None,
                    }),
                    interface_name: Some(InterfaceName {
                        name: "api".to_string(),
                        version: None,
                    }),
                    function_name: "initialize-cart".to_string(),
                }),
                vec![u64()],
                Some(str()),
            ),
            usage_count: 1,
        }],
    }
}

pub(crate) fn get_random_worker_functions_constraint() -> FunctionConstraints {
    FunctionConstraints {
        constraints: vec![FunctionUsageConstraint {
            function_signature: FunctionSignature::new(
                FunctionName::Function(FullyQualifiedFunctionName {
                    package_name: None,
                    interface_name: None,
                    function_name: "foo".to_string(),
                }),
                vec![],
                Some(list(record(vec![
                    NameTypePair {
                        name: "bar".to_string(),
                        typ: str(),
                    },
                    NameTypePair {
                        name: "baz".to_string(),
                        typ: str(),
                    },
                ]))),
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
