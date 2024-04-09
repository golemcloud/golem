use crate::expression::{ConstructorPattern, Expr};
use crate::parser::expr_parser::{parse_tokens, Context};
use crate::parser::ParseError;
use crate::tokeniser::tokenizer::{Token, Tokenizer};

// To parse expressions such as some(x), foo(bar)
pub(crate) fn get_constructor_pattern(
    tokenizer: &mut Tokenizer,
    constructor_name: &str,
) -> Result<ConstructorPattern, ParseError> {
    match tokenizer.next_non_empty_token() {
        Some(Token::LParen) => {
            let constructor_var_optional =
                tokenizer.capture_string_until_and_skip_end(&Token::RParen);
            match constructor_var_optional {
                Some(constructor_var) => {
                    let mut tokenizer = Tokenizer::new(constructor_var.as_str());

                    let constructor_expr = parse_tokens(&mut tokenizer, Context::Code)?;

                    match constructor_expr {
                        Expr::Variable(variable) => ConstructorPattern::constructor(
                            constructor_name.to_string().as_str(),
                            vec![ConstructorPattern::Literal(Box::new(Expr::Variable(
                                variable,
                            )))],
                        ),
                        Expr::Constructor0(pattern) => ConstructorPattern::constructor(
                            constructor_name.to_string().as_str(),
                            vec![pattern],
                        ),
                        expr => ConstructorPattern::constructor(
                            constructor_name.to_string().as_str(),
                            vec![ConstructorPattern::Literal(Box::new(expr))],
                        ),
                    }
                }
                None => Err(ParseError::Message(format!(
                    "Empty value inside the constructor {}",
                    constructor_name
                ))),
            }
        }

        _ => Err(ParseError::Message(format!(
            "Expecting an open parenthesis '(' after {}",
            constructor_name
        ))),
    }
}
