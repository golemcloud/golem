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

use crate::instance_type::InstanceType;
use crate::{Expr, TypeInternal};

// Note that this takes an entire rib program and not any invalid expression
pub fn check_invalid_program_return(rib_program: &Expr) -> Result<(), InvalidProgramReturn> {
    let inferred_type = rib_program.inferred_type();

    if let TypeInternal::Instance { instance_type, .. } = inferred_type.internal_type() {
        let expr = match rib_program {
            Expr::ExprBlock { exprs, .. } if !exprs.is_empty() => exprs.last().unwrap(),
            expr => expr,
        };

        return match instance_type.as_ref() {
            InstanceType::Resource { .. } => Err(InvalidProgramReturn {
                return_expr: expr.clone(),
                message: "program is invalid as it returns a resource constructor".to_string(),
            }),

            _ => Err(InvalidProgramReturn {
                return_expr: expr.clone(),
                message: "program is invalid as it returns a worker instance".to_string(),
            }),
        };
    }

    Ok(())
}

pub struct InvalidProgramReturn {
    pub return_expr: Expr,
    pub message: String,
}
