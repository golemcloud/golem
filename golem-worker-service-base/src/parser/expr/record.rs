use crate::expression::Expr;
use crate::parser::expr::util;
use crate::parser::expr_parser::{parse_with_context, Context};
use crate::parser::ParseError;
use crate::tokeniser::tokenizer::{MultiCharTokens, Token, Tokenizer};

pub(crate) fn create_record(tokenizer: &mut Tokenizer) -> Result<Expr, ParseError> {
    let mut record: Vec<(String, Expr)> = vec![];

    fn go(tokenizer: &mut Tokenizer, record: &mut Vec<(String, Expr)>) -> Result<(), ParseError> {
        match tokenizer.next_non_empty_token() {
            Some(Token::MultiChar(MultiCharTokens::Other(key))) => {
                match tokenizer.next_non_empty_token() {
                    Some(Token::Colon) => {
                        let possible_nestedness = tokenizer.peek_next_non_empty_token();
                        match possible_nestedness {
                            Some(start_token @ Token::LCurly)
                            | Some(start_token @ Token::LParen)
                            | Some(start_token @ Token::LSquare) => {
                                tokenizer.skip_next_non_empty_token(); // Skip the nested token
                                let closing_token = util::get_closing_token(&start_token).ok_or(
                                    ParseError::Message(
                                        "Expecting a closing token for nested record".to_string(),
                                    ),
                                )?;

                                let captured_string = tokenizer.capture_string_until_and_skip_end(
                                    vec![&start_token],
                                    &closing_token,
                                );

                                match captured_string {
                                    Some(captured_string) => {
                                        let full_expr = format!(
                                            "{}{}{}",
                                            &start_token, &captured_string, &closing_token
                                        );

                                        // dbg!(full_expr.clone());

                                        let expr =
                                            parse_with_context(full_expr.as_str(), Context::Code)?;
                                        //  dbg!(expr.clone());
                                        record.push((key.to_string(), expr.clone()));
                                        tokenizer.skip_if_next_non_empty_token_is(&Token::Comma); // Skip comma before looping
                                        go(tokenizer, record)
                                    }
                                    None => Err(ParseError::Message(
                                        "Expecting a value after colon in record 1".to_string(),
                                    )),
                                }
                            }
                            _ => {
                                let captured_value =
                                    tokenizer.capture_string_until(vec![], &Token::Comma);
                                match captured_value {
                                    Some(value) => {
                                        let expr =
                                            parse_with_context(value.as_str(), Context::Code)?;
                                        record.push((key.to_string(), expr.clone()));
                                        tokenizer.next_non_empty_token(); // Skip next comma
                                        go(tokenizer, record)
                                    }
                                    None => {
                                        let last_value = tokenizer.capture_string_until(
                                            vec![&Token::LCurly],
                                            &Token::RCurly,
                                        );

                                        dbg!(&last_value);

                                        match last_value {
                                            Some(last_value) => {
                                                let expr = parse_with_context(
                                                    last_value.as_str(),
                                                    Context::Code,
                                                )?;
                                                record.push((key.to_string(), expr));
                                                Ok(())
                                            }
                                            None => Err(ParseError::Message(
                                                "Expecting a value after colon in record"
                                                    .to_string(),
                                            )),
                                        }
                                    }
                                }
                            }
                        }
                    }
                    _ => Err(ParseError::Message(
                        "Expecting a colon after key in record".to_string(),
                    )),
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
