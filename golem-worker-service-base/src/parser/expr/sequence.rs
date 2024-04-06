use crate::expression::Expr;
use crate::parser::expr::util;
use crate::parser::expr_parser::{parse_with_context, Context};
use crate::parser::ParseError;
use crate::tokeniser::tokenizer::{Token, Tokenizer};

// Assuming the tokenizer already consumed `[` token, indicating the start of sequence
pub(crate) fn create_sequence(tokenizer: &mut Tokenizer) -> Result<Expr, ParseError> {
    let mut record = vec![];

    fn go(tokenizer: &mut Tokenizer, record: &mut Vec<Expr>) -> Result<(), ParseError> {
        let next_token = tokenizer.peek_next_non_empty_token();

        if util::is_next_token_complex_type(tokenizer) {
            let complex_value_start_token = next_token.unwrap();
            accumulate_complex_value_and_continue(tokenizer, complex_value_start_token, record, go)
        } else {
            // Considering the element until the next comma to be primitive/simple values
            let possible_sequence_elem = tokenizer.capture_string_until(
                vec![],
                &Token::Comma, // Wave does this
            );

            match possible_sequence_elem {
                Some(sequence_element_string) => {
                    let sequence_elem_expr =
                        parse_with_context(sequence_element_string.as_str(), Context::Code)?;
                    record.push(sequence_elem_expr);
                    tokenizer.next_non_empty_token();
                    go(tokenizer, record)
                }

                None => {
                    let last_value =
                        tokenizer.capture_string_until(vec![&Token::LSquare], &Token::RSquare);

                    match last_value {
                        Some(last_value) => {
                            let expr = parse_with_context(last_value.as_str(), Context::Code)?;
                            record.push(expr);
                            Ok(())
                        }
                        None => Ok(()),
                    }
                }
            }
        }
    }

    go(tokenizer, &mut record)?;

    Ok(Expr::Sequence(record))
}

// Handling sequence of complex types such as tuple, record, or list itself.
fn accumulate_complex_value_and_continue<F>(
    tokenizer: &mut Tokenizer,
    complex_value_start_token: Token,
    record: &mut Vec<Expr>,
    continue_parsing: F,
) -> Result<(), ParseError>
where
    F: Fn(&mut Tokenizer, &mut Vec<Expr>) -> Result<(), ParseError>,
{
    let closing_token = util::get_closing_token(&complex_value_start_token).ok_or(
        ParseError::Message("Expecting a closing token for nested record".to_string()),
    )?;

    // Skip the opening token
    tokenizer.skip_next_non_empty_token();

    // Capture until the closing token
    let captured_string = tokenizer
        .capture_string_until_and_skip_end(vec![&complex_value_start_token], &closing_token);

    match captured_string {
        Some(captured_string) => {
            // Reconstruct the full expression with opening and closing token
            let full_expr = format!(
                "{}{}{}",
                &complex_value_start_token, &captured_string, &closing_token
            );

            let expr = parse_with_context(full_expr.as_str(), Context::Code)?;
            record.push(expr);
            match tokenizer.peek_next_non_empty_token() {
                Some(Token::Comma) => {
                    tokenizer.skip_next_non_empty_token(); // Skip comma before looping
                    continue_parsing(tokenizer, record)
                }
                Some(Token::RSquare) => {
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
