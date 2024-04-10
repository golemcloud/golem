use crate::expression::{ConstructorPattern, Expr, PatternMatchArm};
use crate::parser::expr::constructor;
use crate::parser::expr_parser::parse_code;
use crate::parser::ParseError;
use crate::tokeniser::tokenizer::{MultiCharTokens, Token, Tokenizer};

pub(crate) fn create_pattern_match_expr(tokenizer: &mut Tokenizer) -> Result<Expr, ParseError> {
    let match_expr = tokenizer.capture_string_until(&Token::LCurly);

    match match_expr {
        Some(expr_under_evaluation) => {
            let match_expression = parse_code(expr_under_evaluation.as_str())?;

            tokenizer.skip_next_non_empty_token(); // Skip LCurly

            let constructors = get_arms(tokenizer)?;

            Ok(Expr::PatternMatch(Box::new(match_expression), constructors))
        }

        None => Err(ParseError::Message(
            "Expecting a valid expression after match".to_string(),
        )),
    }
}

// To parse collection of terms under match expression
// Ex: some(x) => x
// Handles Ok, Err, Some, None
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
            let index_of_closed_curly_brace = tokenizer.index_of_end_token(&Token::RCurly);
            let index_of_commaseparator = tokenizer.index_of_end_token(&Token::Comma);

            match (index_of_closed_curly_brace, index_of_commaseparator) {
                (Some(end_of_constructors), Some(comma)) => {
                    if end_of_constructors > comma {
                        let captured_string = tokenizer.capture_string_until(&Token::Comma);

                        let individual_expr = parse_code(captured_string.unwrap().as_str())
                            .map(|expr| PatternMatchArm((constructor_pattern, Box::new(expr))))?;
                        collected_exprs.push(individual_expr);
                        tokenizer.next_non_empty_token(); // Skip CommaSeparator
                        accumulator(tokenizer, collected_exprs)
                    } else {
                        // End of constructor
                        let captured_string = tokenizer.capture_string_until(&Token::RCurly);
                        let individual_expr = parse_code(captured_string.unwrap().as_str())
                            .map(|expr| PatternMatchArm((constructor_pattern, Box::new(expr))))?;
                        collected_exprs.push(individual_expr);
                        Ok(())
                    }
                }

                // Last constructor
                (Some(_), None) => {
                    let captured_string = tokenizer.capture_string_until(&Token::RCurly);

                    if let Some(captured_string) = captured_string {
                        let individual_expr = parse_code(captured_string.as_str())
                            .map(|expr| PatternMatchArm((constructor_pattern, Box::new(expr))))?;
                        collected_exprs.push(individual_expr);
                    }

                    Ok(())
                }

                _ => Err(ParseError::Message(
                    "Invalid constructor pattern".to_string(),
                )),
            }
        }
        _ => Err(ParseError::Message(
            "Expecting an arrow after Some expression".to_string(),
        )),
    }
}
