use crate::expression::{ConstructorPattern};
use crate::parser::{expr_parser::parse_code, ParseError};
use crate::tokeniser::tokenizer::{Token, Tokenizer};

// To parse expressions such as some(x), foo(bar)
pub(crate) fn get_constructor_pattern(
    tokenizer: &mut Tokenizer,
    constructor_name: &str,
) -> Result<ConstructorPattern, ParseError> {
    if tokenizer.peek_next_non_empty_token_is(&Token::LParen) {
        tokenizer.skip_next_non_empty_token(); // Skip LParen
        let construction_variables =
            match tokenizer.capture_string_until_and_skip_end(&Token::RParen) {
                Some(value) => collect_construction_variables(value.as_str())?,
                None => {
                    return Err(ParseError::Message(format!(
                        "Empty value inside the constructor {}",
                        constructor_name
                    )))
                }
            };
        ConstructorPattern::constructor(constructor_name, construction_variables)
    } else {
        ConstructorPattern::constructor(constructor_name, vec![])
    }
}

fn collect_construction_variables(
    constructor_variable_str: &str,
) -> Result<Vec<ConstructorPattern>, ParseError> {
    let mut tokenizer = Tokenizer::new(constructor_variable_str);
    let mut construction_variables = vec![];
    loop {
        if let Some(value) = tokenizer.capture_string_until_and_skip_end(&Token::Comma) {
            let expr = parse_code(value.as_str())?;
            let constructor_pattern = ConstructorPattern::from_expr(expr);
            construction_variables.push(constructor_pattern);
        } else {
            let rest = tokenizer.rest();
            if !rest.is_empty() {
                let expr = parse_code(rest)?;
                let constructor_pattern = ConstructorPattern::from_expr(expr);
                construction_variables.push(constructor_pattern);
            }
            break;
        }
    }

    Ok(construction_variables)
}
