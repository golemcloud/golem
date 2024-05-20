use crate::expression::Expr;
use crate::parser::ParseError;
use crate::tokeniser::tokenizer::{MultiCharTokens, Token, Tokenizer};

pub fn get_select_index(
    tokenizer: &mut Tokenizer,
    selection_on: &Expr,
) -> Result<Expr, ParseError> {
    match selection_on {
        Expr::Sequence(_)
        | Expr::Record(_)
        | Expr::Variable(_)
        | Expr::SelectField(_, _)
        | Expr::Request()
        | Expr::Worker() => {
            //
            let optional_possible_index = tokenizer.capture_string_until(&Token::RSquare);

            match optional_possible_index {
                Some(index) => {
                    if let Ok(index) = index.trim().parse::<usize>() {
                        Ok(Expr::SelectIndex(Box::new(selection_on.clone()), index))
                    } else {
                        Err(ParseError::Message(format!(
                            "Invalid index {} obtained within square brackets",
                            index
                        )))
                    }
                }
                None => Err(ParseError::Message(
                    "Expecting a valid index inside square brackets near to field".to_string(),
                )),
            }
        }
        other => Err(ParseError::Message(format!(
            "Selecting index is only allowed on sequence or record types. But found {:?}",
            other
        ))),
    }
}

pub fn get_select_field(tokenizer: &mut Tokenizer, selection_on: Expr) -> Result<Expr, ParseError> {
    // If a dot appears, then that means next token is probably a "field" selection rather than expression on its own
    // and cannot delegate to further loops without peeking ahead using tokenizer and attaching the field to the current expression
    let next_token = tokenizer.next_non_empty_token();

    let possible_field = match next_token {
        Some(Token::MultiChar(MultiCharTokens::StringLiteral(field))) => field,
        Some(token) => {
            return Err(ParseError::Message(format!(
                "Expecting a valid field selection after dot instead of {}.",
                token
            )))
        }
        None => {
            return Err(ParseError::Message(
                "Expecting a field after dot".to_string(),
            ))
        }
    };

    Ok(Expr::SelectField(Box::new(selection_on), possible_field))
}
