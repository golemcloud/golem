use golem_wasm_ast::analysis::AnalysedType;
use crate::Expr;
use crate::type_checker::{Path, PathElem};

pub fn find_missing_fields(expr: &Expr, expected: &AnalysedType) -> Vec<Path> {
    let mut missing_paths = Vec::new();

    if let AnalysedType::Record(expected_record) = expected {
        for (field_name, expected_type_of_field) in expected_record
            .fields
            .iter()
            .map(|name_typ| (name_typ.name.clone(), name_typ.typ.clone()))
        {
            if let Expr::Record(actual_reord, _) = expr {
                let actual_value_opt = actual_reord
                    .iter()
                    .find(|(name, _)| *name == field_name)
                    .map(|(_, value)| value);

                if let Some(actual_value) = actual_value_opt {
                    if let AnalysedType::Record(record) = expected_type_of_field {
                        let nested_paths = find_missing_fields(
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