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

mod model;
pub use model::*;

/// Protobuf representation of analysis results
#[cfg(feature = "protobuf")]
pub mod protobuf;

/// Wave format support for types.
///
/// This module is optional and can be enabled with the `metadata` feature flag. It is enabled by default.
#[cfg(feature = "wave")]
pub mod wave;
#[cfg(feature = "wit-parser")]
pub mod wit_parser;

use crate::component::*;
use crate::core::Mem;
use crate::AstCustomization;
use mappable_rc::Mrc;
use std::cell::RefCell;
use std::fmt::Debug;
use std::rc::Rc;

pub type AnalysisResult<A> = Result<A, AnalysisFailure>;

#[derive(Debug, Clone)]
struct ComponentStackItem<Ast: AstCustomization + 'static> {
    component: Mrc<Component<Ast>>,
}

#[derive(Debug, Clone)]
pub struct AnalysisContext<Ast: AstCustomization + 'static> {
    component_stack: Vec<ComponentStackItem<Ast>>,
    warnings: Rc<RefCell<Vec<AnalysisWarning>>>,
}

impl<Ast: AstCustomization + 'static> AnalysisContext<Ast> {
    pub fn new(component: Component<Ast>) -> AnalysisContext<Ast> {
        AnalysisContext::from_rc(Mrc::new(component))
    }

    /// Initializes an analyzer for a given component
    pub fn from_rc(component: Mrc<Component<Ast>>) -> AnalysisContext<Ast> {
        AnalysisContext {
            component_stack: vec![ComponentStackItem { component }],
            warnings: Rc::new(RefCell::new(Vec::new())),
        }
    }

    /// Gets all the memories (not just the exported ones) from all modules within the WASM component
    pub fn get_all_memories(&self) -> AnalysisResult<Vec<Mem>> {
        let mut result = Vec::new();

        let mut component_stack = vec![self.get_component()];
        while let Some(component) = component_stack.pop() {
            for module in component.modules() {
                for mem in module.mems() {
                    result.push((*mem).clone());
                }
            }
            for inner_component in component.components() {
                component_stack.push(inner_component.clone());
            }
        }
        Ok(result)
    }

    pub fn warnings(&self) -> Vec<AnalysisWarning> {
        self.warnings.borrow().clone()
    }

    fn get_component(&self) -> Mrc<Component<Ast>> {
        self.component_stack.last().unwrap().component.clone()
    }
}

#[cfg(test)]
mod tests {
    use crate::analysis::analysed_type::{f32, field, handle, record, result, str, u32, u64};
    use crate::analysis::{
        AnalysedFunction, AnalysedFunctionParameter, AnalysedFunctionResult, AnalysedResourceId,
        AnalysedResourceMode,
    };
    use test_r::test;

    #[test]
    fn analysed_function_kind() {
        let cons = AnalysedFunction {
            name: "[constructor]cart".to_string(),
            parameters: vec![AnalysedFunctionParameter {
                name: "user-id".to_string(),
                typ: str(),
            }],
            result: Some(AnalysedFunctionResult {
                typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Owned),
            }),
        };
        let method = AnalysedFunction {
            name: "[method]cart.add-item".to_string(),
            parameters: vec![
                AnalysedFunctionParameter {
                    name: "self".to_string(),
                    typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                },
                AnalysedFunctionParameter {
                    name: "item".to_string(),
                    typ: record(vec![
                        field("product-id", str()),
                        field("name", str()),
                        field("price", f32()),
                        field("quantity", u32()),
                    ]),
                },
            ],
            result: None,
        };
        let static_method = AnalysedFunction {
            name: "[static]cart.merge".to_string(),
            parameters: vec![
                AnalysedFunctionParameter {
                    name: "self".to_string(),
                    typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                },
                AnalysedFunctionParameter {
                    name: "that".to_string(),
                    typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Borrowed),
                },
            ],
            result: Some(AnalysedFunctionResult {
                typ: handle(AnalysedResourceId(0), AnalysedResourceMode::Owned),
            }),
        };
        let fun = AnalysedFunction {
            name: "hash".to_string(),
            parameters: vec![AnalysedFunctionParameter {
                name: "path".to_string(),
                typ: str(),
            }],
            result: Some(AnalysedFunctionResult {
                typ: result(
                    record(vec![field("lower", u64()), field("upper", u64())]),
                    str(),
                ),
            }),
        };

        assert!(cons.is_constructor());
        assert!(method.is_method());
        assert!(static_method.is_static_method());
        assert!(!fun.is_constructor());
        assert!(!fun.is_method());
        assert!(!fun.is_static_method());
    }
}
