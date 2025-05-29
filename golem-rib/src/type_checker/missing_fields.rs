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

use crate::type_checker::{Path, PathElem};
use crate::Expr;
use golem_wasm_ast::analysis::AnalysedType;

pub fn find_missing_fields_in_record(expr: &Expr, expected: &AnalysedType) -> Vec<Path> {
    let mut missing_paths = Vec::new();

    if let AnalysedType::Record(expected_record) = expected {
        for (field_name, expected_type_of_field) in expected_record
            .fields
            .iter()
            .map(|name_typ| (name_typ.name.clone(), name_typ.typ.clone()))
        {
            if let Expr::Record { exprs, .. } = expr {
                let actual_value_opt = exprs
                    .iter()
                    .find(|(name, _)| *name == field_name)
                    .map(|(_, value)| value);

                if let Some(actual_value) = actual_value_opt {
                    if let AnalysedType::Record(record) = expected_type_of_field {
                        let nested_paths = find_missing_fields_in_record(
                            actual_value,
                            &AnalysedType::Record(record.clone()),
                        );
                        for mut nested_path in nested_paths {
                            // Prepend the current field to the path for each missing nested field
                            nested_path.push_front(PathElem::Field(field_name.clone()));
                            missing_paths.push(nested_path);
                        }
                    }
                } else {
                    missing_paths.push(Path::from_elem(PathElem::Field(field_name.clone())));
                }
            }
        }
    }

    missing_paths
}
