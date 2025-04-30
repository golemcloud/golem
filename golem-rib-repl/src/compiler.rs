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

use crate::repl_state::ReplState;
use golem_wasm_ast::analysis::{TypeEnum, TypeVariant};
use rib::*;
use std::collections::HashMap;
use std::fmt::Display;

pub fn compile_rib_script(
    rib_script: &str,
    repl_state: &mut ReplState,
) -> Result<CompilerOutput, RibCompilationError> {
    let expr = Expr::from_text(rib_script)
        .map_err(|e| RibCompilationError::InvalidSyntax(e.to_string()))?;

    let function_registry =
        FunctionTypeRegistry::from_export_metadata(&repl_state.dependency().metadata);

    let inferred_expr = InferredExpr::from_expr(expr, &function_registry, &vec![])
        .map_err(RibCompilationError::RibTypeError)?;

    let instance_variables = fetch_instance_variables(&inferred_expr);

    let identifiers = get_identifiers(&inferred_expr);

    let variants = function_registry.get_variants();
    let enums = function_registry.get_enums();

    let new_byte_code = RibByteCode::from_expr(&inferred_expr)
        .map_err(|e| RibCompilationError::RibStaticAnalysisError(e.to_string()))?;

    let byte_code = new_byte_code.diff(repl_state.byte_code());

    repl_state.update_byte_code(new_byte_code);

    Ok(CompilerOutput {
        rib_byte_code: byte_code,
        instance_variables,
        identifiers,
        variants,
        enums,
    })
}

#[derive(Clone)]
pub struct CompilerOutput {
    pub rib_byte_code: RibByteCode,
    pub instance_variables: InstanceVariables,
    pub identifiers: Vec<VariableId>,
    pub variants: Vec<TypeVariant>,
    pub enums: Vec<TypeEnum>,
}

#[derive(Default, Clone)]
pub struct InstanceVariables {
    pub instance_variables: HashMap<InstanceKey, FunctionDictionary>,
}

impl InstanceVariables {
    pub fn instance_keys(&self) -> Vec<String> {
        self.instance_variables
            .keys()
            .map(|k| k.to_string())
            .collect()
    }

    pub fn get_worker_instance_method_dict(
        &self,
        instance_key: &str,
    ) -> Option<&FunctionDictionary> {
        self.instance_variables
            .get(&InstanceKey::Worker(instance_key.to_string()))
    }

    pub fn get_resource_instance_method_dict(
        &self,
        instance_key: &str,
    ) -> Option<&FunctionDictionary> {
        self.instance_variables
            .get(&InstanceKey::Resource(instance_key.to_string()))
    }
}

#[derive(Hash, Clone, PartialEq, Eq)]
pub enum InstanceKey {
    Worker(String),
    Resource(String),
}

impl Display for InstanceKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstanceKey::Worker(key) => write!(f, "{}", key),
            InstanceKey::Resource(key) => write!(f, "{}", key),
        }
    }
}

impl InstanceVariables {
    pub fn variable_names(&self) -> Vec<String> {
        self.instance_variables
            .keys()
            .map(|k| k.to_string())
            .collect()
    }

    pub fn method_names(&self) -> Vec<String> {
        self.instance_variables
            .values()
            .flat_map(|dict| dict.function_names())
            .collect()
    }
}

pub fn get_identifiers(inferred_expr: &InferredExpr) -> Vec<VariableId> {
    let mut expr = inferred_expr.get_expr().clone();
    let mut visitor = ExprVisitor::bottom_up(&mut expr);

    let mut identifiers = Vec::new();

    while let Some(expr) = visitor.pop_back() {
        match expr {
            Expr::Let { variable_id, .. } => {
                if !identifiers.contains(variable_id) {
                    identifiers.push(variable_id.clone());
                }
            }
            Expr::Identifier { variable_id, .. } => {
                if !identifiers.contains(variable_id) {
                    identifiers.push(variable_id.clone());
                }
            }
            _ => {}
        }
    }

    identifiers
}

pub fn fetch_instance_variables(inferred_expr: &InferredExpr) -> InstanceVariables {
    let mut expr = inferred_expr.get_expr().clone();
    let mut queue = ExprVisitor::bottom_up(&mut expr);

    let mut instance_variables = HashMap::new();

    while let Some(expr) = queue.pop_front() {
        if let Expr::Let {
            variable_id, expr, ..
        } = expr
        {
            if let TypeInternal::Instance { instance_type } = expr.inferred_type().internal_type() {
                match instance_type.as_ref() {
                    InstanceType::Resource { .. } => {
                        let key = InstanceKey::Resource(variable_id.name());
                        instance_variables.insert(key, instance_type.resource_method_dictionary());
                    }
                    _ => {
                        let key = InstanceKey::Worker(variable_id.name());
                        instance_variables
                            .insert(key, instance_type.function_dict_without_resource_methods());
                    }
                };
            }
        }
    }

    InstanceVariables { instance_variables }
}
