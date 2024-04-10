use crate::expression::{ConstructorPattern, Expr};
use crate::parser::{expr_parser::parse_code, ParseError};
use crate::tokeniser::tokenizer::{Token, Tokenizer};

// To parse expressions such as some(x), foo(bar)
pub(crate) fn get_constructor_pattern(
    tokenizer: &mut Tokenizer,
    constructor_name: &str,
) -> Result<ConstructorPattern, ParseError> {
    match tokenizer.next_non_empty_token() {
        Some(Token::LParen) => {
            let construction_variables = match tokenizer.capture_string_until_and_skip_end(&Token::RParen) {
                Some(value) => collect_construction_variables(value.as_str())?,
                None => return Err(ParseError::Message(format!("Empty value inside the constructor {}", constructor_name))),
            };
            let constructor_patterns = construction_variables
                .into_iter()
                .map(|expr| ConstructorPattern::Literal(Box::new(expr)))
                .collect();
            ConstructorPattern::constructor(constructor_name, constructor_patterns)
        }
        _ => ConstructorPattern::constructor(constructor_name, vec![]),
    }
}

fn collect_construction_variables(constructor_variable_str: &str) -> Result<Vec<Expr>, ParseError> {
    let mut tokenizer = Tokenizer::new(constructor_variable_str);
    let mut construction_variables = vec![];
    loop {
        if let Some(value) = tokenizer.capture_string_until_and_skip_end(&Token::Comma) {
            construction_variables.push(parse_code(value.as_str())?);
        } else {
            let rest = tokenizer.rest();
            if !rest.is_empty() {
                construction_variables.push(parse_code(rest)?);
            }
            break;
        }
    }
    Ok(construction_variables)
}
