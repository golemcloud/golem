use crate::expression::Expr;
use crate::parser::expr_parser::parse_code;
use crate::parser::ParseError;
use crate::tokeniser::tokenizer::{Token, Tokenizer};

// Assuming if token is already consumed
pub(crate) fn create_if_condition(tokenizer: &mut Tokenizer) -> Result<Expr, ParseError> {
    if let Some(predicate_str) =
        tokenizer.capture_string_until_and_skip_end(&Token::then())
    {
        let predicate_expr = parse_code(predicate_str.as_str())?;
        dbg!(&predicate_expr);
        let then_str = tokenizer.capture_string_until_and_skip_end(&Token::else_token());
        match then_str {
            Some(then_str) => {
                let then_expr = parse_code(then_str.as_str())?;

                dbg!(&then_expr);

                let else_condition = tokenizer
                    .capture_string_until_and_skip_end(&Token::SemiColon)
                    .or_else(|| Some(tokenizer.consume_rest().to_string()));

                match else_condition {
                    Some(else_condition) => {
                        let else_expr = parse_code(else_condition.as_str())?;
                        dbg!(&else_expr);
                        Ok(Expr::Cond(
                            Box::new(predicate_expr),
                            Box::new(then_expr),
                            Box::new(else_expr),
                        ))
                    }
                    None => Err(ParseError::Message(
                        "Expecting a valid expression after then".to_string(),
                    )),
                }
            }
            None => Err(ParseError::Message(
                "Expecting a valid expression after then".to_string(),
            )),
        }
    } else {
        Err(ParseError::Message(
            "Expecting a valid expression after if".to_string(),
        ))
    }
}
