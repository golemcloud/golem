use crate::type_inference::kind::TypeKind;
use crate::{Expr, InferredType};
use std::collections::VecDeque;

// Check all exprs that cannot be the type it is tagged against
pub fn check_invalid_expr(expr: &Expr) -> Result<(), InvalidExpr> {
    let mut queue = VecDeque::new();
    queue.push_back(expr);

    while let Some(expr) = queue.pop_back() {
        match expr {
            Expr::Number { inferred_type, .. } => match inferred_type.as_number() {
                Ok(_) => {}
                Err(msg) => {
                    return Err(InvalidExpr {
                        expr: expr.clone(),
                        expected_type: TypeKind::Number,
                        found: inferred_type.clone(),
                        message: msg,
                    });
                }
            },
            _ => expr.visit_children_bottom_up(&mut queue),
        }
    }

    Ok(())
}

#[derive(Debug, Clone)]
pub struct InvalidExpr {
    pub expr: Expr,
    pub expected_type: TypeKind,
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

        let error_message = compile(&expr, &vec![]).unwrap_err();

        let expected = r#"
        error in the following rib found at line 2, column 30
        `1`
        cause: inferred a number to be `list<u32>` which is invalid
        expected a number type, found list
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
