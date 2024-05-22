use crate::expression::Expr;
use crate::parser::expr_parser::parse_code;
use crate::parser::ParseError;
use crate::tokeniser::tokenizer::{Token, Tokenizer};

pub fn get_params(params: &str) -> Result<Vec<Expr>, ParseError> {
    let mut tokenizer = Tokenizer::new(params);
    let mut params = vec![];

    loop {
        if let Some(value) = tokenizer.capture_string_until_and_skip_end(&Token::Comma) {
            let param = parse_code(value.as_str())?;
            params.push(param);
        } else {
            if let Some(rest) = tokenizer.rest_opt() {
                let remaining_param = parse_code(rest)?;
                params.push(remaining_param);
            }

            break;
        }
    }

    Ok(params)
}
