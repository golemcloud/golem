use crate::expression::{ArmPattern, Expr, MatchArm};
use crate::parser::expr_parser::{parse_code};
use crate::parser::ParseError;
use crate::tokeniser::tokenizer::{Token, Tokenizer};

pub(crate) fn create_pattern_match_expr(tokenizer: &mut Tokenizer) -> Result<Expr, ParseError> {
    let match_expr_str = tokenizer
        .capture_string_until(&Token::LCurly)
        .ok_or_else(|| {
            ParseError::Message("Expecting a valid expression after match".to_string())
        })?;

    let match_expression = parse_code(match_expr_str.as_str())?;
    tokenizer.skip_next_non_empty_token(); // Skip LCurly
    let arms = accumulate_match_arms(tokenizer)?;
    Ok(Expr::PatternMatch(Box::new(match_expression), arms))
}

pub(crate) fn accumulate_match_arms(
    tokenizer: &mut Tokenizer,
) -> Result<Vec<MatchArm>, ParseError> {
    let mut match_arms = Vec::new();

    loop {
        let arm_pattern = get_arm_pattern(tokenizer)?;
        let ArmBody { cursor, arm_body } = ArmBody::from_tokenizer(tokenizer)?;
        let complete_arm = MatchArm((arm_pattern, arm_body));
        match_arms.push(complete_arm);

        if cursor == Cursor::Break {
            break;
        }
    }

    if match_arms.is_empty() {
        return Err(ParseError::Message(
            format!("Expecting a constructor pattern, but found nothing at {}", tokenizer.pos())
        ));
    }

    Ok(match_arms)
}

pub(crate) fn get_arm_pattern(tokenizer: &mut Tokenizer) -> Result<ArmPattern, ParseError> {
    if let Some(constructor_name) = tokenizer.next_non_empty_token() {
        match constructor_name {
            Token::WildCard => Ok(ArmPattern::WildCard),
            _ => {
                if tokenizer.peek_next_non_empty_token_is(&Token::LParen) {
                    tokenizer.skip_next_non_empty_token(); // Skip LParen
                    match tokenizer.capture_string_until_and_skip_end(&Token::RParen) {
                        Some(constructor_str) => {
                            let patterns = collect_arm_pattern_variables(constructor_str.as_str())?;
                            dbg!(&patterns);
                            ArmPattern::from(constructor_name.to_string().as_str(), patterns)
                        }
                        None => Err(ParseError::Message(
                            "Empty value inside the constructor".to_string(),
                        )),
                    }
                } else if tokenizer.peek_next_non_empty_token_is(&Token::At) {
                    let variable = constructor_name.to_string();
                    tokenizer.skip_next_non_empty_token(); // Skip At
                    let arm_pattern = get_arm_pattern(tokenizer)?;
                    Ok(ArmPattern::As(variable, Box::new(arm_pattern)))
                } else {
                    ArmPattern::from(constructor_name.to_string().as_str(), vec![])
                }
            }
        }
    } else {
        Err(ParseError::Message(
            "Expecting a constructor name".to_string(),
        ))
    }
}

fn collect_arm_pattern_variables(
    constructor_variable_str: &str,
) -> Result<Vec<ArmPattern>, ParseError> {
    let mut tokenizer = Tokenizer::new(constructor_variable_str);
    let mut arm_patterns = vec![];
    loop {
        if let Some(value) = tokenizer.capture_string_until_and_skip_end(&Token::Comma) {
            let arm_pattern = parse_code(value.as_str())
                .map(ArmPattern::from_expr)
                .or_else(|_| {
                    let mut tokenizer = Tokenizer::new(value.as_str());
                    get_arm_pattern(&mut tokenizer)
                })?;
            arm_patterns.push(arm_pattern);
        } else {
            let rest = tokenizer.rest();
            if !rest.is_empty() {
                dbg!(&rest);

                let arm_pattern = {
                    let mut tokenizer = Tokenizer::new(rest);
                    get_arm_pattern(&mut tokenizer)
                }?;

                dbg!(arm_pattern.clone());
                arm_patterns.push(arm_pattern);
            }
            break;
        }
    }

    Ok(arm_patterns)
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
                let arm = parse_code(captured_string.as_str())?;

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
