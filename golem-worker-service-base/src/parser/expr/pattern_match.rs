use crate::expression::{Expr, PatternMatchArm};
use crate::parser::expr::constructor;
use crate::parser::expr_parser::parse_code;
use crate::parser::ParseError;
use crate::tokeniser::tokenizer::{MultiCharTokens, Token, Tokenizer};

pub(crate) fn create_pattern_match_expr(tokenizer: &mut Tokenizer) -> Result<Expr, ParseError> {
    let match_expr_str = tokenizer
        .capture_string_until(&Token::LCurly)
        .ok_or_else(|| {
            ParseError::Message("Expecting a valid expression after match".to_string())
        })?;

    let match_expression = parse_code(match_expr_str.as_str())?;
    tokenizer.skip_next_non_empty_token(); // Skip LCurly
    let arms = accumulate_arms(tokenizer)?;
    Ok(Expr::PatternMatch(Box::new(match_expression), arms))
}

pub(crate) fn accumulate_arms(
    tokenizer: &mut Tokenizer,
) -> Result<Vec<PatternMatchArm>, ParseError> {
    let mut constructor_patterns = Vec::new();

    while let Some(arm_pattern_token) = tokenizer.next_non_empty_token() {
        let constructor_pattern = constructor::get_constructor_pattern(
            tokenizer,
            arm_pattern_token.to_string().as_str(),
        )?;
        let ArmBody { cursor, arm_body } = ArmBody::from_tokenizer(tokenizer)?;
        let arm = PatternMatchArm((constructor_pattern, arm_body));
        constructor_patterns.push(arm);

        if cursor == Cursor::Break {
            break;
        }
    }

    if constructor_patterns.is_empty() {
        return Err(ParseError::Message(
            "Expecting a constructor pattern, but found nothing".to_string(),
        ));
    }

    Ok(constructor_patterns)
}

#[derive(Debug, PartialEq)]
enum Cursor {
    Continue,
    Break,
}

#[derive(Debug, PartialEq)]
struct ArmBody {
    cursor: Cursor,
    arm_body: Box<Expr>,
}

impl ArmBody {
    fn from_tokenizer(tokenizer: &mut Tokenizer) -> Result<ArmBody, ParseError> {
        if tokenizer.next_non_empty_token_is(&Token::arrow()) {
            if let Some((end_token, captured_string)) =
                tokenizer.capture_string_until_either(&Token::Comma, &Token::RCurly)
            {
                let arm = parse_code(captured_string)?;

                let cursor = if end_token == &Token::RCurly {
                    Cursor::Break
                } else {
                    tokenizer.skip_next_non_empty_token(); // Skip comma
                    Cursor::Continue
                };

                Ok(ArmBody {
                    cursor,
                    arm_body: Box::new(arm),
                })
            } else {
                Err(ParseError::Message(
                    "Expecting an arm body after Some arrow".to_string(),
                ))
            }
        } else {
            Err(ParseError::Message(
                "Expecting an arrow after Some expression".to_string(),
            ))
        }
    }
}
