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

use crate::type_inference::TypeHint;
use crate::{Expr, ExprVisitor, InferredType, TypeInternal};

// Check all exprs that cannot be the type it is tagged against
pub fn check_invalid_expr(expr: &mut Expr) -> Result<(), InvalidExpr> {
    let mut visitor = ExprVisitor::bottom_up(expr);

    while let Some(expr) = visitor.pop_back() {
        if let Expr::Number { inferred_type, .. } = &expr {
            match inferred_type.as_number() {
                Ok(_) => {}
                Err(message) => {
                    return Err(InvalidExpr {
                        expr: expr.clone(),
                        expected_type: TypeHint::Number,
                        found: inferred_type.clone(),
                        message,
                    });
                }
            }
        }
    }

    Ok(())
}

#[derive(Debug, Clone)]
pub struct InvalidExpr {
    pub expr: Expr,
    pub expected_type: TypeHint,
    pub found: InferredType,
    pub message: String,
}

#[cfg(test)]
mod tests {

    use crate::type_checker::invalid_expr::tests::internal::strip_spaces;
    use crate::{compile, Expr};
    use test_r::test;

    #[test]
    fn test_invalid_expression() {
        let expr = r#"
          let x: list<u32> = 1;
          x
        "#;

        let expr = Expr::from_text(expr).unwrap();

        let error_message = compile(expr, &vec![]).unwrap_err().to_string();

        let expected = r#"
        error in the following rib found at line 2, column 30
        `1`
        cause: expected to be of the type `number`, but inferred as `list<u32>`
        used as list
        "#;

        assert_eq!(error_message, strip_spaces(expected));
    }

    mod internal {
        pub(crate) fn strip_spaces(input: &str) -> String {
            let lines = input.lines();

            let first_line = lines
                .clone()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("");
            let margin_width = first_line.chars().take_while(|c| c.is_whitespace()).count();

            let result = lines
                .map(|line| {
                    if line.trim().is_empty() {
                        String::new()
                    } else {
                        line[margin_width..].to_string()
                    }
                })
                .collect::<Vec<String>>()
                .join("\n");

            result.strip_prefix("\n").unwrap_or(&result).to_string()
        }
    }
}
