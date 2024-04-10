use crate::expression::{ConstructorPattern, Expr};
use crate::parser::expr_parser::parse_code;
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
                Some(construction_variables) => {
                    if construction_variables.is_empty() {
                        ConstructorPattern::constructor(
                            constructor_name.to_string().as_str(),
                            vec![],
                        )
                    } else {
                        let construction_variables =
                            collect_construction_variables(construction_variables.as_str())?;
                        let constructor_patterns = construction_variables
                            .iter()
                            .map(|expr| ConstructorPattern::Literal(Box::new(expr.clone())))
                            .collect::<Vec<ConstructorPattern>>();

                        ConstructorPattern::constructor(constructor_name, constructor_patterns)
                    }
                }
                None => Err(ParseError::Message(format!(
                    "Empty value inside the constructor {}",
                    constructor_name
                ))),
            }
        }

        _ => ConstructorPattern::constructor(constructor_name.to_string().as_str(), vec![]),
    }
}

fn collect_construction_variables(constructor_variable_str: &str) -> Result<Vec<Expr>, ParseError> {
    let mut tokenizer = Tokenizer::new(constructor_variable_str);
    let mut construction_variables = vec![];
    loop {
        if let Some(value) = tokenizer.capture_string_until_and_skip_end(&Token::Comma) {
            let construction_variable = parse_code(value.as_str())?;
            construction_variables.push(construction_variable);
        } else {
            let rest = tokenizer.rest();

            if !rest.is_empty() {
                let construction_variable = parse_code(rest)?;
                construction_variables.push(construction_variable);
            }
            break;
        }
    }

    Ok(construction_variables)
}
