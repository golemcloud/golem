use crate::expression::{ConstructorPattern, Expr, PatternMatchArm};
use crate::parser::expr::constructor;
use crate::parser::expr_parser::parse_code;
use crate::parser::ParseError;
use crate::tokeniser::tokenizer::{MultiCharTokens, Token, Tokenizer};

pub(crate) fn create_pattern_match_expr(tokenizer: &mut Tokenizer) -> Result<Expr, ParseError> {
    let match_expr_str = tokenizer.capture_string_until(&Token::LCurly)
        .ok_or_else(|| ParseError::Message("Expecting a valid expression after match".to_string()))?;

    let match_expression = parse_code(match_expr_str.as_str())?;
    tokenizer.skip_next_non_empty_token(); // Skip LCurly
    let constructors = get_arms(tokenizer)?;
    Ok(Expr::PatternMatch(Box::new(match_expression), constructors))
}


pub(crate) fn get_arms(tokenizer: &mut Tokenizer) -> Result<Vec<PatternMatchArm>, ParseError> {
    let mut constructor_patterns: Vec<PatternMatchArm> = vec![];

    fn go(
        tokenizer: &mut Tokenizer,
        constructor_patterns: &mut Vec<PatternMatchArm>,
    ) -> Result<(), ParseError> {
        match tokenizer.next_non_empty_token() {
            Some(token) => {
                let constructor_pattern =
                    constructor::get_constructor_pattern(tokenizer, token.to_string().as_str())?;

                accumulate_arms(tokenizer, constructor_pattern, constructor_patterns, go)
            }

            None => Err(ParseError::Message(
                "Expecting a constructor pattern. But found nothing".to_string(),
            )),
        }
    }

    go(tokenizer, &mut constructor_patterns)?;
    Ok(constructor_patterns)
}

fn accumulate_arms<F>(
    tokenizer: &mut Tokenizer,
    constructor_pattern: ConstructorPattern,
    collected_exprs: &mut Vec<PatternMatchArm>,
    accumulator: F,
) -> Result<(), ParseError>
    where
        F: FnOnce(&mut Tokenizer, &mut Vec<PatternMatchArm>) -> Result<(), ParseError>,
{
    match tokenizer.next_non_empty_token() {
        Some(Token::MultiChar(MultiCharTokens::Arrow)) => {
            if let Some((end_token, captured_string)) = tokenizer.capture_string_until_either(&Token::Comma, &Token::RCurly) {
                let individual_expr = parse_code(captured_string)
                    .map(|expr| PatternMatchArm((constructor_pattern, Box::new(expr))))?;
                collected_exprs.push(individual_expr);

                if end_token == &Token::RCurly {
                    Ok(())
                } else {
                    tokenizer.skip_next_non_empty_token(); // Skip comma
                    accumulator(tokenizer, collected_exprs)
                }
            } else {
                Ok(())
            }
        }
        _ => Err(ParseError::Message(
            "Expecting an arrow after Some expression".to_string(),
        )),
    }
}
