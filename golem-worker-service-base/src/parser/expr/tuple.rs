use crate::expression::Expr;
use crate::parser::expr_parser::parse_code;
use crate::parser::ParseError;
use crate::tokeniser::tokenizer::{Token, Tokenizer};

// Assuming the tokenizer already consumed `(` token, indicating the start of tuple
pub(crate) fn create_tuple(tokenizer: &mut Tokenizer) -> Result<Expr, ParseError> {
    let mut tuple_elements = vec![];
    let mut grouped_exprs: Vec<Expr> = vec![];

    fn go(
        tokenizer: &mut Tokenizer,
        tuple_elements: &mut Vec<Expr>,
        grouped_exprs: &mut Vec<Expr>,
    ) -> Result<(), ParseError> {
        let captured_string = tokenizer.capture_string_until(
            &Token::Comma, // Wave does this
        );

        match captured_string {
            Some(r) => {
                let expr = parse_code(r.as_str())?;

                tuple_elements.push(expr);
                tokenizer.next_non_empty_token(); // Skip Comma
                go(tokenizer, tuple_elements, grouped_exprs)
            }

            None => {
                let last_value = tokenizer.capture_string_until_and_skip_end(&Token::RParen);

                match last_value {
                    Some(last_value) if !last_value.is_empty() => {
                        let expr = parse_code(last_value.as_str())?;
                        // If there is only 1 element, and it's an invalid tuple element, then we need to push it to grouped_exprs
                        if tuple_elements.is_empty() && !is_valid_tuple_element(&expr) {
                            grouped_exprs.push(expr.clone());
                        }

                        tuple_elements.push(expr);

                        Ok(())
                    }
                    Some(_) => Ok(()),
                    None => Err(ParseError::Message(
                        "Expecting a closing paren (RParen)".to_string(),
                    )),
                }
            }
        }
    }

    go(tokenizer, &mut tuple_elements, &mut grouped_exprs)?;

    if !grouped_exprs.is_empty() {
        Ok(grouped_exprs.pop().unwrap())
    } else {
        Ok(Expr::Tuple(tuple_elements))
    }
}

// We allow only WIT types to be part of the tuple
// and not expressions such as Expr::Cond, or pattern matching
// However complex expressions can be contained within () to
// allow grouping of expressions
fn is_valid_tuple_element(expr: &Expr) -> bool {
    match expr {
        Expr::Identifier(_) => true,
        Expr::Call(_, _) => false,
        Expr::Number(_) => true,
        Expr::Boolean(_) => true,
        Expr::Flags(_) => true,
        Expr::Record(_) => true,
        Expr::Sequence(_) => true,
        Expr::Concat(_) => true,
        Expr::Option(_) => true,
        Expr::Result(_) => true,
        Expr::Tuple(_) => true,
        Expr::Literal(_) => true,
        Expr::Not(_) => false,
        Expr::Request() => false,
        Expr::Worker() => false,
        Expr::SelectField(_, _) => false,
        Expr::SelectIndex(_, _) => false,
        Expr::Cond(_, _, _) => false, // we disallow if statements within tuple
        Expr::PatternMatch(_, _) => false,
        Expr::GreaterThan(_, _) => false,
        Expr::LessThan(_, _) => false,
        Expr::EqualTo(_, _) => false,
        Expr::GreaterThanOrEqualTo(_, _) => false,
        Expr::LessThanOrEqualTo(_, _) => false,
        Expr::Let(_, _) => false,
        Expr::Multiple(_) => false,
    }
}
