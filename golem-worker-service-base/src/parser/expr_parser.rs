use std::rc::Rc;

use crate::expression::{ConstructorPattern, Expr};
use crate::tokeniser::cursor::TokenCursor;
use crate::tokeniser::tokenizer::{Token, TokeniserResult, Tokenizer};

use super::*;
use internal::*;

#[derive(Clone, Debug)]
pub struct ExprParser {}

#[derive(Clone, Debug)]
enum Context {
    Code,
    Literal,
}

impl Context {
    fn is_code(&self) -> bool {
        match self {
            Context::Code => true,
            Context::Literal => false,
        }
    }
}

impl GolemParser<Expr> for ExprParser {
    fn parse(&self, input: &str) -> Result<Expr, ParseError> {
        parse_with_context(input, Context::Literal)
    }
}

fn parse_with_context(input: &str, context: Context) -> Result<Expr, ParseError> {
    let tokeniser_result: TokeniserResult = tokenise(input);

    parse_tokens(tokeniser_result, context)
}

fn parse_tokens(tokeniser_result: TokeniserResult, context: Context) -> Result<Expr, ParseError> {
    fn go(
        cursor: &mut TokenCursor,
        context: Context,
        prev_expression: InternalExprResult,
    ) -> Result<Expr, ParseError> {
        let token = if context.is_code() {
            cursor.next_non_empty_token()
        } else {
            cursor.next_token().map(|t| t.as_raw_string_token())
        };

        if let Some(token) = token {
            match token {
                Token::RawString(raw_string) => {
                    let new_expr = if context.is_code() {
                        resolve_literal_in_code_context(raw_string.as_str())
                    } else {
                        Expr::Literal(raw_string)
                    };

                    go(cursor, context, prev_expression.apply_with(new_expr))
                }

                Token::Request => go(cursor, context, prev_expression.apply_with(Expr::Request())),

                Token::None => go(
                    cursor,
                    context,
                    prev_expression.apply_with(Expr::Constructor0(
                        ConstructorPattern::constructor("none", vec![])?,
                    )),
                ),

                token @ Token::Some | token @ Token::Ok | token @ Token::Err => {
                    match cursor.next_non_empty_token() {
                        Some(Token::OpenParen) => {
                            let constructor_var_optional = cursor
                                .capture_string_until_and_skip_end(
                                    vec![&Token::OpenParen],
                                    &Token::CloseParen,
                                );
                            let constructor = match constructor_var_optional {
                                Some(constructor_var) => {
                                    let mut cursor =
                                        Tokenizer::new(constructor_var.as_str()).run().to_cursor();

                                    let constructor_expr =
                                        go(&mut cursor, Context::Code, InternalExprResult::Empty)?;

                                    match constructor_expr {
                                        Expr::Variable(variable) => {
                                            ConstructorPattern::constructor(
                                                token.to_string().as_str(),
                                                vec![ConstructorPattern::Literal(Box::new(
                                                    Expr::Variable(variable),
                                                ))],
                                            )
                                        }
                                        Expr::Constructor0(pattern) => {
                                            ConstructorPattern::constructor(
                                                token.to_string().as_str(),
                                                vec![pattern],
                                            )
                                        }
                                        expr => ConstructorPattern::constructor(
                                            token.to_string().as_str(),
                                            vec![ConstructorPattern::Literal(Box::new(expr))],
                                        ),
                                    }
                                }
                                None => Err(ParseError::Message(format!(
                                    "Empty value inside the constructor {}",
                                    token
                                ))),
                            };

                            go(
                                cursor,
                                context,
                                prev_expression.apply_with(Expr::Constructor0(constructor?)),
                            )
                        }

                        _ => Err(ParseError::Message(format!(
                            "Expecting an open parenthesis '(' after {}",
                            token
                        ))),
                    }
                }

                Token::Worker => go(cursor, context, prev_expression.apply_with(Expr::Worker())),

                Token::InterpolationStart => {
                    let new_expr = capture_expression_until(
                        cursor,
                        vec![&Token::InterpolationStart, &Token::OpenCurlyBrace],
                        Some(&Token::ClosedCurlyBrace),
                        prev_expression,
                        go,
                    )?;

                    go(cursor, context, new_expr)
                }

                Token::Quote => {
                    let non_code_string =
                        cursor.capture_string_until_and_skip_end(vec![], &Token::Quote);

                    let new_expr = match non_code_string {
                        Some(string) => {
                            let mut cursor = Tokenizer::new(string.as_str()).run().to_cursor();

                            go(&mut cursor, Context::Literal, InternalExprResult::Empty)
                        }
                        None => Err(ParseError::Message(
                            "Expecting a non-empty string between quotes".to_string(),
                        )),
                    };

                    go(cursor, context, prev_expression.apply_with(new_expr?))
                }

                Token::OpenParen => {
                    let expr = capture_expression_until(
                        cursor,
                        vec![&Token::OpenParen],
                        Some(&Token::CloseParen),
                        prev_expression,
                        go,
                    )?;

                    go(cursor, context, expr)
                }

                Token::GreaterThanOrEqualTo => {
                    if prev_expression.is_empty() {
                        return Err(ParseError::Message(
                            "GreaterThanOrEqualTo (>=) is applied to a non existing left expression"
                                .to_string(),
                        ));
                    };

                    let new_expr = build_with_last_complete_expr(
                        ExpressionContext::GreaterThanOrEqualTo,
                        prev_expression,
                        |prev, new| {
                            InternalExprResult::complete(Expr::GreaterThanOrEqualTo(
                                Box::new(prev),
                                Box::new(new),
                            ))
                        },
                    )?;

                    go(cursor, context, new_expr)
                }

                Token::GreaterThan => {
                    if prev_expression.is_empty() {
                        return Err(ParseError::Message(
                            "GreaterThan (>) is applied to a non existing left expression"
                                .to_string(),
                        ));
                    };

                    let new_expr = build_with_last_complete_expr(
                        ExpressionContext::GreaterThan,
                        prev_expression,
                        |prev, new| {
                            InternalExprResult::complete(Expr::GreaterThan(
                                Box::new(prev),
                                Box::new(new),
                            ))
                        },
                    )?;

                    go(cursor, context, new_expr)
                }

                Token::LessThan => {
                    if prev_expression.is_empty() {
                        return Err(ParseError::Message(
                            "LessThan (<) is applied to a non existing left expression".to_string(),
                        ));
                    };

                    let new_expr = build_with_last_complete_expr(
                        ExpressionContext::LessThan,
                        prev_expression,
                        |prev, new| {
                            InternalExprResult::complete(Expr::LessThan(
                                Box::new(prev),
                                Box::new(new),
                            ))
                        },
                    )?;

                    go(cursor, context, new_expr)
                }

                Token::LessThanOrEqualTo => {
                    if prev_expression.is_empty() {
                        return Err(ParseError::Message(
                            "LessThanOrEqualTo (<=)  is applied to a non existing left expression"
                                .to_string(),
                        ));
                    };

                    let new_expr = build_with_last_complete_expr(
                        ExpressionContext::LessThanOrEqualTo,
                        prev_expression,
                        |prev, new| {
                            InternalExprResult::complete(Expr::LessThanOrEqualTo(
                                Box::new(prev),
                                Box::new(new),
                            ))
                        },
                    )?;

                    go(cursor, context, new_expr)
                }

                Token::EqualTo => {
                    if prev_expression.is_empty() {
                        return Err(ParseError::Message(
                            "EqualTo (=) is applied to a non existing left expression".to_string(),
                        ));
                    };

                    let new_expr = build_with_last_complete_expr(
                        ExpressionContext::EqualTo,
                        prev_expression,
                        |prev, new| {
                            InternalExprResult::complete(Expr::EqualTo(
                                Box::new(prev),
                                Box::new(new),
                            ))
                        },
                    )?;

                    go(cursor, context, new_expr)
                }

                Token::Dot => {
                    // If a dot appears, then that means next token is probably a "field" selection rather than expression on its own
                    // and cannot delegate to further loops without peeking ahead using cursor and attaching the field to the current expression
                    let next_token = cursor.next_non_empty_token();

                    let possible_field = match next_token {
                        Some(Token::RawString(field)) => field,
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

                    match prev_expression {
                        InternalExprResult::Complete(expr) => go(
                            cursor,
                            context,
                            InternalExprResult::complete(Expr::SelectField(
                                Box::new(expr),
                                possible_field,
                            )),
                        ),

                        _ => Err(ParseError::Message(format!(
                            "Invalid token field {}. Make sure expression format is correct",
                            possible_field
                        ))),
                    }
                }

                Token::OpenSquareBracket => match prev_expression {
                    InternalExprResult::Complete(prev_expr) => {
                        let optional_possible_index = cursor.capture_string_until(
                            vec![&Token::OpenSquareBracket],
                            &Token::ClosedSquareBracket,
                        );

                        match optional_possible_index {
                            Some(index) => {
                                if let Ok(index) = index.trim().parse::<usize>() {
                                    go(
                                        cursor,
                                        context,
                                        InternalExprResult::complete(Expr::SelectIndex(
                                            Box::new(prev_expr),
                                            index,
                                        )),
                                    )
                                } else {
                                    Err(ParseError::Message(format!(
                                        "Invalid index {} obtained within square brackets",
                                        index
                                    )))
                                }
                            }
                            None => Err(ParseError::Message(
                                "Expecting a valid index inside square brackets near to field"
                                    .to_string(),
                            )),
                        }
                    }

                    _ => Err(ParseError::Message("Invalid token [".to_string())),
                },

                Token::If => {
                    // We expect to form Expr::Cond given three unknown variables
                    let future_expr = InternalExprResult::incomplete(
                        ExpressionContext::Condition,
                        move |first_result| {
                            let first_result: Rc<Expr> = Rc::new(first_result);
                            InternalExprResult::incomplete(
                                ExpressionContext::Condition,
                                move |second_result| {
                                    let first_result: Rc<Expr> = Rc::clone(&first_result);
                                    InternalExprResult::incomplete(
                                        ExpressionContext::Condition,
                                        move |else_result| {
                                            let first_result: Expr =
                                                (*Rc::clone(&first_result)).clone();
                                            InternalExprResult::complete(Expr::Cond(
                                                Box::new(first_result),
                                                Box::new(second_result.clone()),
                                                Box::new(else_result),
                                            ))
                                        },
                                    )
                                },
                            )
                        },
                    );

                    let captured_predicate = capture_expression_until(
                        cursor,
                        vec![&Token::If],
                        Some(&Token::Then),
                        future_expr,
                        go,
                    )?;

                    go(cursor, context, captured_predicate)
                }

                Token::Match => {
                    let expr_under_evaluation =
                        cursor.capture_string_until(vec![], &Token::OpenCurlyBrace);

                    let new_expr = match expr_under_evaluation {
                        Some(expr_under_evaluation) => {
                            let mut new_cursor = Tokenizer::new(expr_under_evaluation.as_str())
                                .run()
                                .to_cursor();
                            let expression =
                                go(&mut new_cursor, Context::Code, InternalExprResult::Empty)?;
                            match cursor.next_non_empty_token() {
                                Some(Token::OpenCurlyBrace) => {
                                    let constructors = get_constructors(cursor)?;
                                    Ok(InternalExprResult::complete(Expr::PatternMatch(
                                        Box::new(expression),
                                        constructors,
                                    )))
                                }
                                _ => Err(ParseError::Message(
                                    "Expecting a curly brace after match expr".to_string(),
                                )),
                            }
                        }

                        None => Err(ParseError::Message(
                            "Expecting a valid expression after match".to_string(),
                        )),
                    };

                    go(cursor, context, new_expr?)
                }

                Token::Then => match prev_expression {
                    InternalExprResult::InComplete(ExpressionContext::Condition, _) => {
                        let mew_expr = capture_expression_until(
                            cursor,
                            vec![&Token::Then],
                            Some(&Token::Else),
                            prev_expression,
                            go,
                        )?;

                        go(cursor, context, mew_expr)
                    }

                    _ => Err(ParseError::Message(
                        "then is a keyword and should be part of a if else condition logic"
                            .to_string(),
                    )),
                },

                Token::Else => match prev_expression {
                    InternalExprResult::InComplete(ExpressionContext::Condition, _) => {
                        let expr = capture_expression_until(
                            cursor,
                            vec![&Token::Else],
                            None,
                            prev_expression,
                            go,
                        )?;

                        go(cursor, context, expr)
                    }
                    _ => Err(ParseError::Message(
                        "else is a keyword and should be part of a if else condition logic"
                            .to_string(),
                    )),
                },

                Token::ClosedCurlyBrace => go(cursor, context, prev_expression),
                Token::ClosedSquareBracket => go(cursor, context, prev_expression),
                Token::CloseParen => go(cursor, context, prev_expression),
                Token::Space => go(cursor, context, prev_expression),
                Token::NewLine => go(cursor, context, prev_expression),
                Token::OpenCurlyBrace => go(cursor, context, prev_expression),
                Token::Arrow => go(cursor, context, prev_expression),
                Token::Comma => go(cursor, context, prev_expression),
            }
        } else {
            match prev_expression {
                InternalExprResult::Complete(expr) => Ok(expr),
                _ => Err(ParseError::Message(
                    "failed expression. Internal logical error".to_string(),
                )),
            }
        }
    }

    let mut tokeniser_cursor = tokeniser_result.to_cursor();

    go(&mut tokeniser_cursor, context, InternalExprResult::Empty)
}

mod internal {
    use crate::expression::{ConstructorPattern, ConstructorPatternExpr, Expr, InnerNumber};
    use crate::parser::expr_parser::{parse_with_context, Context};
    use crate::parser::ParseError;
    use crate::tokeniser::cursor::TokenCursor;
    use crate::tokeniser::tokenizer::{Token, TokeniserResult, Tokenizer};
    use strum_macros::Display;

    pub(crate) fn resolve_literal_in_code_context(primitive: &str) -> Expr {
        if let Ok(u64) = primitive.parse::<u64>() {
            Expr::Number(InnerNumber::UnsignedInteger(u64))
        } else if let Ok(i64_value) = primitive.parse::<i64>() {
            Expr::Number(InnerNumber::Integer(i64_value))
        } else if let Ok(f64_value) = primitive.parse::<f64>() {
            Expr::Number(InnerNumber::Float(f64_value))
        } else if let Ok(boolean) = primitive.parse::<bool>() {
            Expr::Boolean(boolean)
        } else {
            Expr::Variable(primitive.to_string())
        }
    }

    pub(crate) fn tokenise(input: &str) -> TokeniserResult {
        Tokenizer::new(input).run()
    }

    // While at every node of Token, we can ideally form a complete expression
    // by peeking ahead using cursor multiple times (an example is peek ahead 3 times for if predicate then then-expr else else-expr),
    // sometimes it is better
    // to defer it to further loop by forming an incomplete expression at a node.
    // Example: At Token::If, we peek ahead only once to get the predicate, and form an incomplete expression
    // which is nothing but a function that takes `then expr` and `else expr` to form the condition expression
    // which will be completed only in further loops.
    pub(crate) enum InternalExprResult {
        Complete(Expr),
        InComplete(ExpressionContext, Box<dyn Fn(Expr) -> InternalExprResult>),
        Empty,
    }

    impl InternalExprResult {
        pub(crate) fn is_empty(&self) -> bool {
            match self {
                InternalExprResult::Complete(_) => false,
                InternalExprResult::InComplete(_, _) => false,
                InternalExprResult::Empty => true,
            }
        }

        pub(crate) fn apply_with(&self, expr: Expr) -> InternalExprResult {
            match self {
                InternalExprResult::Complete(complete_expr) => match complete_expr {
                    Expr::Concat(vec) => {
                        let mut new_expr = vec.clone();
                        new_expr.push(expr);
                        InternalExprResult::complete(Expr::Concat(new_expr))
                    }
                    _ => InternalExprResult::complete(Expr::Concat(vec![
                        complete_expr.clone(),
                        expr,
                    ])),
                },
                InternalExprResult::InComplete(_, in_complete) => in_complete(expr),
                InternalExprResult::Empty => InternalExprResult::Complete(expr),
            }
        }
        pub(crate) fn complete(expr: Expr) -> InternalExprResult {
            InternalExprResult::Complete(expr)
        }

        pub(crate) fn incomplete<F>(scope: ExpressionContext, f: F) -> InternalExprResult
        where
            F: Fn(Expr) -> InternalExprResult + 'static,
        {
            InternalExprResult::InComplete(
                scope,
                Box::new(f) as Box<dyn Fn(Expr) -> InternalExprResult>,
            )
        }
    }

    // The errors that happens in a context can make use of more information in
    // its message
    #[derive(Display)]
    pub(crate) enum ExpressionContext {
        Condition,
        LessThan,
        GreaterThan,
        EqualTo,
        GreaterThanOrEqualTo,
        LessThanOrEqualTo,
    }

    pub(crate) fn get_constructors(
        cursor: &mut TokenCursor,
    ) -> Result<Vec<ConstructorPatternExpr>, ParseError> {
        let mut constructor_patterns: Vec<ConstructorPatternExpr> = vec![];

        fn go(
            cursor: &mut TokenCursor,
            constructor_patterns: &mut Vec<ConstructorPatternExpr>,
        ) -> Result<(), ParseError> {
            match cursor.next_non_empty_token() {
                Some(token) if token.is_non_empty_constructor() => {
                    let next_non_empty_open_braces = cursor.next_non_empty_token();

                    match next_non_empty_open_braces {
                        Some(Token::OpenParen) => {
                            let constructor_var_optional = cursor
                                .capture_string_until_and_skip_end(
                                    vec![&Token::OpenParen],
                                    &Token::CloseParen,
                                );

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
                                        );

                                    accumulate_constructor_pattern_expr(cursor, constructor_pattern?, constructor_patterns, go)

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
                        cursor,
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

        go(cursor, &mut constructor_patterns)?;
        Ok(constructor_patterns)
    }

    pub(crate) fn accumulate_constructor_pattern_expr<F>(
        cursor: &mut TokenCursor,
        constructor_pattern: ConstructorPattern,
        collected_exprs: &mut Vec<ConstructorPatternExpr>,
        accumulator: F,
    ) -> Result<(), ParseError>
    where
        F: FnOnce(&mut TokenCursor, &mut Vec<ConstructorPatternExpr>) -> Result<(), ParseError>,
    {
        match cursor.next_non_empty_token() {
            Some(Token::Arrow) => {
                let index_of_closed_curly_brace = cursor.index_of_last_end_token(
                    vec![&Token::OpenCurlyBrace, &Token::InterpolationStart],
                    &Token::ClosedCurlyBrace,
                );
                let index_of_commaseparator = cursor.index_of_last_end_token(vec![], &Token::Comma);

                match (index_of_closed_curly_brace, index_of_commaseparator) {
                    (Some(end_of_constructors), Some(comma)) => {
                        if end_of_constructors > comma {
                            let captured_string =
                                cursor.capture_string_until(vec![], &Token::Comma);

                            let individual_expr = parse_with_context(
                                captured_string.unwrap().as_str(),
                                Context::Code,
                            )
                            .map(|expr| {
                                ConstructorPatternExpr((constructor_pattern, Box::new(expr)))
                            })?;
                            collected_exprs.push(individual_expr);
                            cursor.next_non_empty_token(); // Skip CommaSeparator
                            accumulator(cursor, collected_exprs)
                        } else {
                            // End of constructor
                            let captured_string = cursor.capture_string_until(
                                vec![&Token::OpenCurlyBrace],
                                &Token::ClosedCurlyBrace,
                            );
                            let individual_expr = parse_with_context(
                                captured_string.unwrap().as_str(),
                                Context::Code,
                            )
                            .map(|expr| {
                                ConstructorPatternExpr((constructor_pattern, Box::new(expr)))
                            })?;
                            collected_exprs.push(individual_expr);
                            Ok(())
                        }
                    }

                    (Some(_), None) => {
                        let captured_string = cursor.capture_string_until(
                            vec![&Token::OpenCurlyBrace, &Token::InterpolationStart],
                            &Token::ClosedCurlyBrace,
                        );

                        let individual_expr =
                            parse_with_context(captured_string.unwrap().as_str(), Context::Code)
                                .map(|expr| {
                                    ConstructorPatternExpr((constructor_pattern, Box::new(expr)))
                                })?;
                        collected_exprs.push(individual_expr);
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
    // possible_nested_token_starts
    // corresponds to the tokens whose closed end is same as capture_until
    // and we should include those capture_untils
    pub(crate) fn capture_expression_until<F>(
        cursor: &mut TokenCursor,
        possible_nested_token_starts: Vec<&Token>,
        capture_until: Option<&Token>,
        future_expression: InternalExprResult,
        get_expr: F,
    ) -> Result<InternalExprResult, ParseError>
    where
        F: FnOnce(&mut TokenCursor, Context, InternalExprResult) -> Result<Expr, ParseError>,
    {
        let optional_captured_string = match capture_until {
            Some(last_token) => {
                cursor.capture_string_until(possible_nested_token_starts, last_token)
            }
            None => cursor.capture_tail(),
        };

        match optional_captured_string {
            Some(captured_string) => {
                let mut new_cursor = Tokenizer::new(captured_string.as_str()).run().to_cursor();

                let inner_expr =
                    get_expr(&mut new_cursor, Context::Code, InternalExprResult::Empty)?;

                Ok(future_expression.apply_with(inner_expr))
            }
            None => Err(ParseError::Message(format!(
                "Unable to find a matching closing symbol {:?}",
                capture_until
            ))),
        }
    }

    // Keep building the expression only if previous expression is a complete expression
    pub(crate) fn build_with_last_complete_expr<F>(
        scope: ExpressionContext,
        last_expression: InternalExprResult,
        complete_expression: F,
    ) -> Result<InternalExprResult, ParseError>
    where
        F: Fn(Expr, Expr) -> InternalExprResult + 'static,
    {
        match last_expression {
            InternalExprResult::Complete(prev_complete_expr) => {
                let new_incomplete_expr = InternalExprResult::incomplete(scope, {
                    move |future_expr| complete_expression(prev_complete_expr.clone(), future_expr)
                });

                Ok(new_incomplete_expr)
            }

            InternalExprResult::InComplete(_, _) => Err(ParseError::Message(
                "Cannot apply greater than on top of an incomplete expression".to_string(),
            )),

            InternalExprResult::Empty => Err(ParseError::Message(
                "Cannot apply greater than on an empty expression".to_string(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expression::ConstructorPatternExpr;

    #[test]
    fn expr_parser_without_vars() {
        let expression_parser = ExprParser {};

        let result = expression_parser.parse("foo");
        let expected = Expr::Literal(String::from("foo"));
        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn expr_parser_with_path() {
        let expression_parser = ExprParser {};

        let result = expression_parser.parse("${request.body.input[0]}");
        let request = Expr::Request();
        let select_body = Expr::SelectField(Box::new(request), "body".to_string());

        let select_input = Expr::SelectField(Box::new(select_body), "input".to_string());

        let first_index = Expr::SelectIndex(Box::new(select_input), 0);

        assert_eq!(result, Ok(first_index));
    }

    #[test]
    fn expr_parser_with_worker_result_path() {
        let expression_parser = ExprParser {};

        let result = expression_parser.parse("${worker.response.input[0]}");
        let worker = Expr::Worker();
        let select_input = Expr::SelectField(
            Box::new(Expr::SelectField(Box::new(worker), "response".to_string())),
            "input".to_string(),
        );
        let first_index = Expr::SelectIndex(Box::new(select_input), 0);

        assert_eq!(result, Ok(first_index));
    }

    #[test]
    fn expr_parser_with_vars() {
        let expression_parser = ExprParser {};

        let result = expression_parser.parse("foo-id-${request.path.user_id}");

        let expected = Expr::Concat(vec![
            Expr::Literal("foo-id-".to_string()),
            Expr::SelectField(
                Box::new(Expr::SelectField(
                    Box::new(Expr::Request()),
                    "path".to_string(),
                )),
                "user_id".to_string(),
            ),
        ]);

        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn expression_with_predicate0() {
        let expression_parser = ExprParser {};

        let result = expression_parser.parse("${1>2}");

        let expected = Expr::GreaterThan(
            Box::new(Expr::unsigned_integer(1)),
            Box::new(Expr::unsigned_integer(2)),
        );

        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn expression_with_predicate000() {
        let expression_parser = ExprParser {};

        let result = expression_parser.parse("${${request.path.user-id} > ${request.path.id}}");

        let expected = Expr::GreaterThan(
            Box::new(Expr::SelectField(
                Box::new(Expr::SelectField(
                    Box::new(Expr::Request()),
                    "path".to_string(),
                )),
                "user-id".to_string(),
            )),
            Box::new(Expr::SelectField(
                Box::new(Expr::SelectField(
                    Box::new(Expr::Request()),
                    "path".to_string(),
                )),
                "id".to_string(),
            )),
        );

        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn expression_with_predicate1() {
        let expression_parser = ExprParser {};

        let result = expression_parser.parse("${${request.path.user-id}>${request.path.id}}");

        let expected = Expr::GreaterThan(
            Box::new(Expr::SelectField(
                Box::new(Expr::SelectField(
                    Box::new(Expr::Request()),
                    "path".to_string(),
                )),
                "user-id".to_string(),
            )),
            Box::new(Expr::SelectField(
                Box::new(Expr::SelectField(
                    Box::new(Expr::Request()),
                    "path".to_string(),
                )),
                "id".to_string(),
            )),
        );

        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn expression_with_predicate2() {
        let expression_parser = ExprParser {};

        let result = expression_parser.parse("${${request.path.user-id}>2}");

        let expected = Expr::GreaterThan(
            Box::new(Expr::SelectField(
                Box::new(Expr::SelectField(
                    Box::new(Expr::Request()),
                    "path".to_string(),
                )),
                "user-id".to_string(),
            )),
            Box::new(Expr::unsigned_integer(2)),
        );

        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn expression_with_predicate_without_outer_interpolation() {
        let expression_parser = ExprParser {};

        let result = expression_parser.parse("${request.path.user-id}>2");

        let expected = Expr::Concat(vec![
            Expr::SelectField(
                Box::new(Expr::SelectField(
                    Box::new(Expr::Request()),
                    "path".to_string(),
                )),
                "user-id".to_string(),
            ),
            Expr::Literal(">".to_string()),
            Expr::Literal("2".to_string()),
        ]);

        assert_eq!(result, Ok(expected));
    }

    #[test]
    fn test_if_expr_with_paranthesis() {
        let expression_parser = ExprParser {};

        let result = expression_parser
            .parse("${if(request.path)then 1 else 0 }")
            .unwrap();

        let expected = Expr::Cond(
            Box::new(Expr::SelectField(
                Box::new(Expr::Request()),
                "path".to_string(),
            )),
            Box::new(Expr::unsigned_integer(1)),
            Box::new(Expr::unsigned_integer(0)),
        );

        assert_eq!(result, expected);
    }

    #[test]
    fn test_if_expr_without_paranthesis() {
        let expression_parser = ExprParser {};

        let result = expression_parser.parse("${if hello then foo else bar}");

        let expected = Expr::Cond(
            Box::new(Expr::Variable("hello".to_string())),
            Box::new(Expr::Variable("foo".to_string())),
            Box::new(Expr::Variable("bar".to_string())),
        );

        assert_eq!(result.unwrap(), expected)
    }

    #[test]
    fn test_if_expr_but_as_literal() {
        let expression_parser = ExprParser {};

        let result = expression_parser.parse("if hello then foo else bar");

        assert!(result.unwrap().is_literal())
    }

    #[test]
    fn test_if_else_then_expr() {
        let expression_parser = ExprParser {};

        let result = expression_parser
            .parse("${if foo then 1 else if bar then 2 else 0}")
            .unwrap();

        // cond(path, 1, cond(2, 2, 0))
        let expected = Expr::Cond(
            Box::new(Expr::Variable("foo".to_string())),
            Box::new(Expr::unsigned_integer(1)),
            Box::new(Expr::Cond(
                Box::new(Expr::Variable("bar".to_string())),
                Box::new(Expr::unsigned_integer(2)),
                Box::new(Expr::unsigned_integer(0)),
            )),
        );

        assert_eq!(result, expected);
    }

    #[test]
    fn test_if_else_then_expr_nested() {
        let expression_parser = ExprParser {};

        let result = expression_parser
            .parse("${if false then 1 else if true then 2 else if false then 1 else 0}")
            .unwrap();

        // cond(path, 1, cond(2, 2, 0))
        let expected = Expr::Cond(
            Box::new(Expr::Boolean(false)),
            Box::new(Expr::unsigned_integer(1)),
            Box::new(Expr::Cond(
                Box::new(Expr::Boolean(true)),
                Box::new(Expr::unsigned_integer(2)),
                Box::new(Expr::Cond(
                    Box::new(Expr::Boolean(false)),
                    Box::new(Expr::unsigned_integer(1)),
                    Box::new(Expr::unsigned_integer(0)),
                )),
            )),
        );

        assert_eq!(result, expected);
    }

    #[test]
    fn test_if_else_then_nested_with_equality_predicates() {
        let expression_parser = ExprParser {};

        let result = expression_parser
            .parse("${if (request.path.user_id == 1) then 1 else if (request.path.user_id == 2) then 2 else if (request.path.user_id == 3) then 3 else 0}")
            .unwrap();

        // cond(path, 1, cond(2, 2, 0))
        let expected = Expr::Cond(
            Box::new(Expr::EqualTo(
                Box::new(Expr::SelectField(
                    Box::new(Expr::SelectField(
                        Box::new(Expr::Request()),
                        "path".to_string(),
                    )),
                    "user_id".to_string(),
                )),
                Box::new(Expr::unsigned_integer(1)),
            )),
            Box::new(Expr::unsigned_integer(1)),
            Box::new(Expr::Cond(
                Box::new(Expr::EqualTo(
                    Box::new(Expr::SelectField(
                        Box::new(Expr::SelectField(
                            Box::new(Expr::Request()),
                            "path".to_string(),
                        )),
                        "user_id".to_string(),
                    )),
                    Box::new(Expr::unsigned_integer(2)),
                )),
                Box::new(Expr::unsigned_integer(2)),
                Box::new(Expr::Cond(
                    Box::new(Expr::EqualTo(
                        Box::new(Expr::SelectField(
                            Box::new(Expr::SelectField(
                                Box::new(Expr::Request()),
                                "path".to_string(),
                            )),
                            "user_id".to_string(),
                        )),
                        Box::new(Expr::unsigned_integer(3)),
                    )),
                    Box::new(Expr::unsigned_integer(3)),
                    Box::new(Expr::unsigned_integer(0)),
                )),
            )),
        );

        assert_eq!(result, expected);
    }

    #[test]
    fn test_if_else_with_path_variable_in_predicate() {
        let expression_parser = ExprParser {};

        let result = expression_parser
            .parse("${if request.path.user_id > 1 then 1 else 0}")
            .unwrap();

        // TODOl Use our own predicate combinators
        let predicate_expressions = Expr::GreaterThan(
            Box::new(Expr::SelectField(
                Box::new(Expr::SelectField(
                    Box::new(Expr::Request()),
                    "path".to_string(),
                )),
                "user_id".to_string(),
            )),
            Box::new(Expr::unsigned_integer(1)),
        );

        let expected = Expr::Cond(
            Box::new(predicate_expressions),
            Box::new(Expr::unsigned_integer(1)),
            Box::new(Expr::unsigned_integer(0)),
        );

        assert_eq!(result, expected);
    }

    #[test]
    fn test_if_else_with_path_variable_in_predicate_and_left() {
        let expression_parser = ExprParser {};

        let result = expression_parser
            .parse("${if request.path.user_id > 1 then request.path.user_id else 0}")
            .unwrap();

        // TODOl Use our own predicate combinators
        let predicate_expressions = Expr::GreaterThan(
            Box::new(Expr::SelectField(
                Box::new(Expr::SelectField(
                    Box::new(Expr::Request()),
                    "path".to_string(),
                )),
                "user_id".to_string(),
            )),
            Box::new(Expr::unsigned_integer(1)),
        );

        let expected = Expr::Cond(
            Box::new(predicate_expressions),
            Box::new(Expr::SelectField(
                Box::new(Expr::SelectField(
                    Box::new(Expr::Request()),
                    "path".to_string(),
                )),
                "user_id".to_string(),
            )),
            Box::new(Expr::unsigned_integer(0)),
        );

        assert_eq!(result, expected);
    }

    #[test]
    fn test_if_else_with_path_variable_in_predicate_left_and_right() {
        let expression_parser = ExprParser {};

        let result = expression_parser
            .parse("${if request.path.user_id > 1 then request.path.user_id else request.path.id}")
            .unwrap();

        // TODOl Use our own predicate combinators
        let predicate_expressions = Expr::GreaterThan(
            Box::new(Expr::SelectField(
                Box::new(Expr::SelectField(
                    Box::new(Expr::Request()),
                    "path".to_string(),
                )),
                "user_id".to_string(),
            )),
            Box::new(Expr::unsigned_integer(1)),
        );

        let expected = Expr::Cond(
            Box::new(predicate_expressions),
            Box::new(Expr::SelectField(
                Box::new(Expr::SelectField(
                    Box::new(Expr::Request()),
                    "path".to_string(),
                )),
                "user_id".to_string(),
            )),
            Box::new(Expr::SelectField(
                Box::new(Expr::SelectField(
                    Box::new(Expr::Request()),
                    "path".to_string(),
                )),
                "id".to_string(),
            )),
        );

        assert_eq!(result, expected);
    }

    #[test]
    fn test_if_expr_with_nested_code() {
        let expression_parser = ExprParser {};

        let result = expression_parser
            .parse("${if request.path.user_id >1 then 0 else (if request.path.user_id == 1 then 0 else 1)}")
            .unwrap();

        let predicate_expressions = Expr::GreaterThan(
            Box::new(Expr::SelectField(
                Box::new(Expr::SelectField(
                    Box::new(Expr::Request()),
                    "path".to_string(),
                )),
                "user_id".to_string(),
            )),
            Box::new(Expr::unsigned_integer(1)),
        );

        let expected = Expr::Cond(
            Box::new(predicate_expressions),
            Box::new(Expr::unsigned_integer(0)),
            Box::new(Expr::Cond(
                Box::new(Expr::EqualTo(
                    Box::new(Expr::SelectField(
                        Box::new(Expr::SelectField(
                            Box::new(Expr::Request()),
                            "path".to_string(),
                        )),
                        "user_id".to_string(),
                    )),
                    Box::new(Expr::unsigned_integer(1)),
                )),
                Box::new(Expr::unsigned_integer(0)),
                Box::new(Expr::unsigned_integer(1)),
            )),
        );

        assert_eq!(result, expected);
    }

    #[test]
    fn test_if_expr_with_complex_nested_code() {
        let expression_parser = ExprParser {};

        let result = expression_parser
            .parse("foo-${if ((if request.path.hello then 1 else 0) > 0) then request.path.user_id else 0}")
            .unwrap();

        // TODOl Use our own predicate combinators
        let predicate_expressions = Expr::GreaterThan(
            Box::new(Expr::Cond(
                Box::new(Expr::SelectField(
                    Box::new(Expr::SelectField(
                        Box::new(Expr::Request()),
                        "path".to_string(),
                    )),
                    "hello".to_string(),
                )),
                Box::new(Expr::unsigned_integer(1)),
                Box::new(Expr::unsigned_integer(0)),
            )),
            Box::new(Expr::unsigned_integer(0)),
        );

        let expected = Expr::Concat(vec![
            Expr::Literal("foo-".to_string()),
            Expr::Cond(
                Box::new(predicate_expressions),
                Box::new(Expr::SelectField(
                    Box::new(Expr::SelectField(
                        Box::new(Expr::Request()),
                        "path".to_string(),
                    )),
                    "user_id".to_string(),
                )),
                Box::new(Expr::unsigned_integer(0)),
            ),
        ]);

        assert_eq!(result, expected);
    }

    #[test]
    fn test_if_expr_with_grouping_predicate() {
        let expression_parser = ExprParser {};

        let result = expression_parser
            .parse("foo-${if (if request.path.hello then 1 else 0) > 0 then request.path.user_id else 0}")
            .unwrap();

        // TODOl Use our own predicate combinators
        let predicate_expressions = Expr::GreaterThan(
            Box::new(Expr::Cond(
                Box::new(Expr::SelectField(
                    Box::new(Expr::SelectField(
                        Box::new(Expr::Request()),
                        "path".to_string(),
                    )),
                    "hello".to_string(),
                )),
                Box::new(Expr::unsigned_integer(1)),
                Box::new(Expr::unsigned_integer(0)),
            )),
            Box::new(Expr::unsigned_integer(0)),
        );

        let expected = Expr::Concat(vec![
            Expr::Literal("foo-".to_string()),
            Expr::Cond(
                Box::new(predicate_expressions),
                Box::new(Expr::SelectField(
                    Box::new(Expr::SelectField(
                        Box::new(Expr::Request()),
                        "path".to_string(),
                    )),
                    "user_id".to_string(),
                )),
                Box::new(Expr::unsigned_integer(0)),
            ),
        ]);

        assert_eq!(result, expected);
    }

    #[test]
    fn test_if_expr_with_pattern_match_variables() {
        let expression_parser = ExprParser {};

        let result = expression_parser
            .parse("${match worker.response { some(foo) => foo, none => result2 } }")
            .unwrap();

        let expected = Expr::PatternMatch(
            Box::new(Expr::SelectField(
                Box::new(Expr::Worker()),
                "response".to_string(),
            )),
            vec![
                ConstructorPatternExpr((
                    ConstructorPattern::constructor(
                        "some",
                        vec![ConstructorPattern::Literal(Box::new(Expr::Variable(
                            "foo".to_string(),
                        )))],
                    )
                    .unwrap(),
                    Box::new(Expr::Variable("foo".to_string())),
                )),
                ConstructorPatternExpr((
                    ConstructorPattern::constructor("none", vec![]).unwrap(),
                    Box::new(Expr::Variable("result2".to_string())),
                )),
            ],
        );

        assert_eq!(result, expected);
    }

    #[test]
    fn test_if_expr_with_pattern_match_variables_ok() {
        let expression_parser = ExprParser {};

        let result = expression_parser
            .parse("${match worker.response { ok(foo) => foo, err(bar) => result2 } }")
            .unwrap();

        let expected = Expr::PatternMatch(
            Box::new(Expr::SelectField(
                Box::new(Expr::Worker()),
                "response".to_string(),
            )),
            vec![
                ConstructorPatternExpr((
                    ConstructorPattern::constructor(
                        "ok",
                        vec![ConstructorPattern::Literal(Box::new(Expr::Variable(
                            "foo".to_string(),
                        )))],
                    )
                    .unwrap(),
                    Box::new(Expr::Variable("foo".to_string())),
                )),
                ConstructorPatternExpr((
                    ConstructorPattern::constructor(
                        "err",
                        vec![ConstructorPattern::Literal(Box::new(Expr::Variable(
                            "bar".to_string(),
                        )))],
                    )
                    .unwrap(),
                    Box::new(Expr::Variable("result2".to_string())),
                )),
            ],
        );

        assert_eq!(result, expected);
    }

    #[test]
    fn test_if_expr_with_pattern_match_variable_and_constants() {
        let expression_parser = ExprParser {};

        let result = expression_parser
            .parse("${match worker.response { some(foo) => worker.response, none => 'nothing' } }")
            .unwrap();

        let expected = Expr::PatternMatch(
            Box::new(Expr::SelectField(
                Box::new(Expr::Worker()),
                "response".to_string(),
            )),
            vec![
                ConstructorPatternExpr((
                    ConstructorPattern::constructor(
                        "some",
                        vec![ConstructorPattern::Literal(Box::new(Expr::Variable(
                            "foo".to_string(),
                        )))],
                    )
                    .unwrap(),
                    Box::new(Expr::SelectField(
                        Box::new(Expr::Worker()),
                        "response".to_string(),
                    )),
                )),
                ConstructorPatternExpr((
                    ConstructorPattern::constructor("none", vec![]).unwrap(),
                    Box::new(Expr::Literal("nothing".to_string())),
                )),
            ],
        );

        assert_eq!(result, expected);
    }

    #[test]
    fn test_if_expr_with_pattern_match_nested_some() {
        let expression_parser = ExprParser {};

        let result = expression_parser
            .parse("${match worker.response { some(some(foo)) => worker.response, none => 'nothing' } }")
            .unwrap();

        let expected = Expr::PatternMatch(
            Box::new(Expr::SelectField(
                Box::new(Expr::Worker()),
                "response".to_string(),
            )),
            vec![
                ConstructorPatternExpr((
                    ConstructorPattern::constructor(
                        "some",
                        vec![ConstructorPattern::constructor(
                            "some",
                            vec![ConstructorPattern::Literal(Box::new(Expr::Variable(
                                "foo".to_string(),
                            )))],
                        )
                        .unwrap()],
                    )
                    .unwrap(),
                    Box::new(Expr::SelectField(
                        Box::new(Expr::Worker()),
                        "response".to_string(),
                    )),
                )),
                ConstructorPatternExpr((
                    ConstructorPattern::constructor("none", vec![]).unwrap(),
                    Box::new(Expr::Literal("nothing".to_string())),
                )),
            ],
        );
        assert_eq!(result, expected);
    }

    #[test]
    fn test_if_expr_with_pattern_match_constants_only() {
        let expression_parser = ExprParser {};

        let result = expression_parser
            .parse("${match worker.response { some(foo) => 'foo', none => 'bar bar' } }")
            .unwrap();

        let expected = Expr::PatternMatch(
            Box::new(Expr::SelectField(
                Box::new(Expr::Worker()),
                "response".to_string(),
            )),
            vec![
                ConstructorPatternExpr((
                    ConstructorPattern::constructor(
                        "some",
                        vec![ConstructorPattern::Literal(Box::new(Expr::Variable(
                            "foo".to_string(),
                        )))],
                    )
                    .unwrap(),
                    Box::new(Expr::Literal("foo".to_string())),
                )),
                ConstructorPatternExpr((
                    ConstructorPattern::constructor("none", vec![]).unwrap(),
                    Box::new(Expr::Concat(vec![
                        Expr::Literal("bar".to_string()),
                        Expr::Literal(" ".to_string()),
                        Expr::Literal("bar".to_string()),
                    ])),
                )),
            ],
        );

        assert_eq!(result, expected);
    }
}
