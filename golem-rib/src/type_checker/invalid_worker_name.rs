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

use crate::call_type::{CallType, InstanceCreationType};
use crate::{Expr, ExprVisitor, InvalidWorkerName};

// Capture all worker name and see if they are resolved to a string type
pub fn check_invalid_worker_name(expr: &mut Expr) -> Result<(), InvalidWorkerName> {
    let mut visitor = ExprVisitor::bottom_up(expr);

    while let Some(expr) = visitor.pop_back() {
        if let Expr::Call { call_type, .. } = expr {
            match call_type {
                CallType::InstanceCreation(InstanceCreationType::Worker { worker_name }) => {
                    internal::check_worker_name(worker_name)?;
                }
                CallType::Function { worker, .. } => {
                    internal::check_worker_name(worker)?;
                }
                CallType::VariantConstructor(_) => {}
                CallType::EnumConstructor(_) => {}
                CallType::InstanceCreation(InstanceCreationType::Resource {
                    worker_name, ..
                }) => {
                    internal::check_worker_name(worker_name)?;
                }
            }
        }
    }

    Ok(())
}

mod internal {
    use crate::type_refinement::precise_types::StringType;
    use crate::type_refinement::TypeRefinement;
    use crate::{Expr, InvalidWorkerName, TypeName};
    use std::ops::Deref;

    pub(crate) fn check_worker_name(
        worker_name: &Option<Box<Expr>>,
    ) -> Result<(), InvalidWorkerName> {
        match worker_name {
            None => {}
            Some(expr) => {
                let inferred_type = expr.inferred_type();
                let string_type = StringType::refine(&inferred_type);

                match string_type {
                    Some(_) => {}
                    None => {
                        let type_name = TypeName::try_from(inferred_type.clone())
                            .map(|t| t.to_string())
                            .unwrap_or_else(|_| "unknown".to_string());
                        return Err(InvalidWorkerName {
                            worker_name_expr: expr.deref().clone(),
                            message: format!("expected string, found {}", type_name),
                        });
                    }
                }
            }
        }

        Ok(())
    }
}
