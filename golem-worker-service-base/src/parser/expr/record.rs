use crate::expression::Expr;
use crate::parser::expr::util;
use crate::parser::expr_parser::{parse_with_context, Context};
use crate::parser::ParseError;
use crate::tokeniser::tokenizer::{MultiCharTokens, Token, Tokenizer};

// Assuming the tokenizer already consumed `{` token, indicating the start of sequence
pub(crate) fn create_record(tokenizer: &mut Tokenizer) -> Result<Expr, ParseError> {
    let mut record: Vec<(String, Expr)> = vec![];

    fn go(tokenizer: &mut Tokenizer, record: &mut Vec<(String, Expr)>) -> Result<(), ParseError> {
        match tokenizer.next_non_empty_token() {
            Some(Token::MultiChar(MultiCharTokens::Other(key))) => {
                if tokenizer.next_non_empty_token_is(&Token::Colon) {
                    if util::is_next_token_complex_type(tokenizer) {
                        let complex_value_start_token =
                            tokenizer.peek_next_non_empty_token().unwrap();
                        accumulate_complex_type_and_continue(
                            tokenizer,
                            record,
                            key,
                            complex_value_start_token,
                            go,
                        )
                    } else {
                        let captured_value = tokenizer.capture_string_until(vec![], &Token::Comma);
                        match captured_value {
                            Some(value) => {
                                let expr = parse_with_context(value.as_str(), Context::Code)?;
                                record.push((key.to_string(), expr.clone()));
                                tokenizer.next_non_empty_token(); // Skip next comma
                                go(tokenizer, record)
                            }
                            None => {
                                let last_value = tokenizer
                                    .capture_string_until(vec![&Token::LCurly], &Token::RCurly);

                                dbg!(&last_value);

                                match last_value {
                                    Some(last_value) => {
                                        let expr =
                                            parse_with_context(last_value.as_str(), Context::Code)?;
                                        record.push((key.to_string(), expr));
                                        Ok(())
                                    }
                                    None => Err(ParseError::Message(
                                        "Expecting a value after colon in record".to_string(),
                                    )),
                                }
                            }
                        }
                    }
                } else {
                    Err(ParseError::Message(
                        "Expecting a colon after key in record".to_string(),
                    ))
                }
            }

            _ => Err(ParseError::Message("Expecting a key in record".to_string())),
        }
    }

    go(tokenizer, &mut record)?;
    Ok(Expr::Record(
        record
            .iter()
            .map(|(s, e)| (s.clone(), Box::new(e.clone())))
            .collect::<Vec<_>>(),
    ))
}

fn accumulate_complex_type_and_continue<F>(
    tokenizer: &mut Tokenizer,
    record: &mut Vec<(String, Expr)>,
    key_of_complex_value: String,
    complex_value_start_token: Token,
    continue_parsing: F,
) -> Result<(), ParseError>
where
    F: Fn(&mut Tokenizer, &mut Vec<(String, Expr)>) -> Result<(), ParseError>,
{
    tokenizer.skip_next_non_empty_token(); // Skip the nested token
    let closing_token = util::get_closing_token(&complex_value_start_token).ok_or(
        ParseError::Message("Expecting a closing token for nested record".to_string()),
    )?;

    let captured_string = tokenizer
        .capture_string_until_and_skip_end(vec![&complex_value_start_token], &closing_token);

    match captured_string {
        Some(captured_string) => {
            let full_expr = format!(
                "{}{}{}",
                &complex_value_start_token, &captured_string, &closing_token
            );

            let expr = parse_with_context(full_expr.as_str(), Context::Code)?;
            record.push((key_of_complex_value, expr.clone()));
            match tokenizer.peek_next_non_empty_token() {
                Some(Token::Comma) => {
                    tokenizer.skip_next_non_empty_token(); // Skip comma before looping
                    continue_parsing(tokenizer, record)
                }
                Some(Token::RCurly) => {
                    tokenizer.skip_next_non_empty_token(); // Skip RSquare before looping
                    Ok(())
                }
                _ => Err(ParseError::Message(
                    "Expecting a comma or closing square bracket".to_string(),
                )),
            }
        }
        None => Err(ParseError::Message(
            "Expecting a value after colon in record 1".to_string(),
        )),
    }
}
