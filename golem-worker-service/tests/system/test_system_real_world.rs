// System test for real-world API definitions
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

use crate::api::definition::types::{ApiDefinition, Route, HttpMethod};
use crate::api::openapi::OpenApiGenerator;

#[test]
fn test_real_world_api_definition() {
    // Step 1: Define a real-world API
    let api_definition = ApiDefinition {
        id: "real_world_api".to_string(),
        name: "Real World API".to_string(),
        version: "1.0.0".to_string(),
        description: "An API with realistic routes".to_string(),
        routes: vec![
            Route {
                path: "/users".to_string(),
                method: HttpMethod::Get,
                description: "Retrieve users".to_string(),
                component_name: "UsersComponent".to_string(),
                binding: BindingType::Default {
                    input_type: AnalysedType::None,
                    output_type: AnalysedType::List(Box::new(model::TypeList {
                        inner: AnalysedType::Record(model::TypeRecord {
                            fields: vec![
                                NameTypePair {
                                    name: "id".to_string(),
                                    typ: AnalysedType::U64(model::TypeU64 {}),
                                },
                                NameTypePair {
                                    name: "name".to_string(),
                                    typ: AnalysedType::Str(model::TypeStr {}),
                                },
                            ],
                        }),
                    })),
                    function_name: "get_users".to_string(),
                },
            },
        ],
    };

    // Step 2: Generate the OpenAPI Spec
    let openapi_spec = OpenApiGenerator::generate(&api_definition);

    // Step 3: Validate OpenAPI Spec structure
    assert!(openapi_spec.paths.contains_key("/users"));
    assert_eq!(openapi_spec.info.title, "Real World API");
    assert_eq!(openapi_spec.info.version, "1.0.0");
}
