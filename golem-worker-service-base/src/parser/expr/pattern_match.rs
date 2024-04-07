use crate::expression::{ConstructorPattern, ConstructorPatternExpr, Expr};
use crate::parser::expr_parser::{parse_with_context, Context};
use crate::parser::ParseError;
use crate::tokeniser::tokenizer::{MultiCharTokens, Token, Tokenizer};

pub(crate) fn get_match_expr(tokenizer: &mut Tokenizer) -> Result<Expr, ParseError> {
    let expr_under_evaluation = tokenizer.capture_string_until(&Token::LCurly);

    match expr_under_evaluation {
        Some(expr_under_evaluation) => {
            let expression = parse_with_context(expr_under_evaluation.as_str(), Context::Code)?;

            match tokenizer.next_non_empty_token() {
                Some(Token::LCurly) => {
                    let constructors = get_match_constructor_patter_exprs(tokenizer)?;

                    Ok(Expr::PatternMatch(Box::new(expression), constructors))
                }
                _ => Err(ParseError::Message(
                    "Expecting a curly brace after match expr".to_string(),
                )),
            }
        }

        None => Err(ParseError::Message(
            "Expecting a valid expression after match".to_string(),
        )),
    }
}

// To parse collection of terms under match expression
// Ex: some(x) => x
// Handles Ok, Err, Some, None
pub(crate) fn get_match_constructor_patter_exprs(
    tokenizer: &mut Tokenizer,
) -> Result<Vec<ConstructorPatternExpr>, ParseError> {
    let mut constructor_patterns: Vec<ConstructorPatternExpr> = vec![];

    fn go(
        tokenizer: &mut Tokenizer,
        constructor_patterns: &mut Vec<ConstructorPatternExpr>,
    ) -> Result<(), ParseError> {
        match tokenizer.next_non_empty_token() {
            Some(token) if token.is_non_empty_constructor() => {
                let next_non_empty_open_braces = tokenizer.next_non_empty_token();

                match next_non_empty_open_braces {
                    Some(Token::LParen) => {
                        let constructor_var_optional =
                            tokenizer.capture_string_until_and_skip_end(&Token::RParen);

                        match constructor_var_optional {
                            Some(constructor_var) => {
                                let expr = parse_with_context(constructor_var.as_str(), Context::Code)?;

                                let cons = match expr {
                                    Expr::Constructor0(cons) => cons,
                                    expr => ConstructorPattern::Literal(Box::new(expr))
                                };

                                let constructor_pattern =
                                    ConstructorPattern::constructor(
                                        token.to_string().as_str(),
                                        vec![cons],
                                    )?;

                                accumulate_constructor_pattern_expr(tokenizer, constructor_pattern, constructor_patterns, go)

                            }
                            _ => Err(ParseError::Message(
                                format!("Token {} is a non empty constructor. Expecting the following pattern: {}(foo) => bar", token, token),
                            )),
                        }
                    }

                    value => Err(ParseError::Message(format!(
                        "Expecting an open parenthesis, but found {}",
                        value.map(|x| x.to_string()).unwrap_or("".to_string())
                    ))),
                }
            }

            Some(token) if token.is_empty_constructor() => {
                let constructor_pattern =
                    ConstructorPattern::constructor(token.to_string().as_str(), vec![]);

                accumulate_constructor_pattern_expr(
                    tokenizer,
                    constructor_pattern?,
                    constructor_patterns,
                    go,
                )
            }

            Some(token) => Err(ParseError::Message(format!(
                "Expecting a constructor pattern. But found {}",
                token
            ))),

            None => Err(ParseError::Message(
                "Expecting a constructor pattern. But found nothing".to_string(),
            )),
        }
    }

    go(tokenizer, &mut constructor_patterns)?;
    Ok(constructor_patterns)
}

fn accumulate_constructor_pattern_expr<F>(
    tokenizer: &mut Tokenizer,
    constructor_pattern: ConstructorPattern,
    collected_exprs: &mut Vec<ConstructorPatternExpr>,
    accumulator: F,
) -> Result<(), ParseError>
where
    F: FnOnce(&mut Tokenizer, &mut Vec<ConstructorPatternExpr>) -> Result<(), ParseError>,
{
    match tokenizer.next_non_empty_token() {
        Some(Token::MultiChar(MultiCharTokens::Arrow)) => {
            let index_of_closed_curly_brace = tokenizer.index_of_end_token(&Token::RCurly);
            let index_of_commaseparator = tokenizer.index_of_end_token(&Token::Comma);

            match (index_of_closed_curly_brace, index_of_commaseparator) {
                (Some(end_of_constructors), Some(comma)) => {
                    if end_of_constructors > comma {
                        let captured_string = tokenizer.capture_string_until(&Token::Comma);

                        let individual_expr =
                            parse_with_context(captured_string.unwrap().as_str(), Context::Code)
                                .map(|expr| {
                                    ConstructorPatternExpr((constructor_pattern, Box::new(expr)))
                                })?;
                        collected_exprs.push(individual_expr);
                        tokenizer.next_non_empty_token(); // Skip CommaSeparator
                        accumulator(tokenizer, collected_exprs)
                    } else {
                        // End of constructor
                        let captured_string = tokenizer.capture_string_until(&Token::RCurly);
                        let individual_expr =
                            parse_with_context(captured_string.unwrap().as_str(), Context::Code)
                                .map(|expr| {
                                    ConstructorPatternExpr((constructor_pattern, Box::new(expr)))
                                })?;
                        collected_exprs.push(individual_expr);
                        Ok(())
                    }
                }

                // Last constructor
                (Some(_), None) => {
                    let captured_string = tokenizer.capture_string_until(&Token::RCurly);

                    if let Some(captured_string) = captured_string {
                        let individual_expr =
                            parse_with_context(captured_string.as_str(), Context::Code).map(
                                |expr| {
                                    ConstructorPatternExpr((constructor_pattern, Box::new(expr)))
                                },
                            )?;
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
