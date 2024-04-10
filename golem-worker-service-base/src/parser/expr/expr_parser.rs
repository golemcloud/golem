use std::rc::Rc;

use crate::expression::Expr;
use crate::tokeniser::tokenizer::{MultiCharTokens, Token, Tokenizer};

use crate::parser::expr::{constructor, flags, if_condition, let_statement, pattern_match, quoted, record, selection, sequence, tuple, util};
use crate::parser::{GolemParser, ParseError};
use internal::*;

#[derive(Clone, Debug)]
pub struct ExprParser {}

impl GolemParser<Expr> for ExprParser {
    fn parse(&self, input: &str) -> Result<Expr, ParseError> {
        parse_text(input) // Everything is a string unless interpolated
    }
}

// parse_as_str and parse_as_code avoids the strong dependency to
// `${` and `}`.
pub(crate) fn parse_text(input: &str) -> Result<Expr, ParseError> {
    let mut tokenizer: Tokenizer = Tokenizer::new(input);

    let mut expressions: Vec<Expr> = vec![];

    while let Some(token) = tokenizer.next_token() {
        match token {
            Token::MultiChar(MultiCharTokens::InterpolationStart) => {
                let captured_string = tokenizer.capture_string_until_and_skip_end(&Token::RCurly);

                if let Some(captured_string) = captured_string {
                    let current_expr = parse_code(captured_string.as_str())?;
                    expressions.push(current_expr);
                }
            }
            token => {
                let expr = Expr::Literal(token.to_string());
                expressions.push(expr);
            }
        }
    }

    if expressions.len() == 1 {
        Ok(expressions[0].clone())
    } else {
        Ok(Expr::Concat(expressions))
    }
}

pub(crate) fn parse_code(input: impl AsRef<str>) -> Result<Expr, ParseError> {
    fn go(
        tokenizer: &mut Tokenizer,
        prev_expression: InternalExprResult,
    ) -> Result<Expr, ParseError> {
        if let Some(token) = tokenizer.next_non_empty_token() {
            match token {
                Token::MultiChar(MultiCharTokens::Other(raw_string)) => {
                    let new_expr = get_expr_from_custom_string(tokenizer, raw_string.as_str())?;

                    go(tokenizer, prev_expression.apply_with(new_expr))
                }

                Token::MultiChar(MultiCharTokens::Number(number)) => {
                    let new_expr = util::get_primitive_expr(number.as_str());
                    go(tokenizer, prev_expression.apply_with(new_expr))
                }

                Token::MultiChar(MultiCharTokens::Request) => {
                    go(tokenizer, prev_expression.apply_with(Expr::Request()))
                }

                token @ Token::MultiChar(MultiCharTokens::Some)
                | token @ Token::MultiChar(MultiCharTokens::None)
                | token @ Token::MultiChar(MultiCharTokens::Ok)
                | token @ Token::MultiChar(MultiCharTokens::Err) => {
                    let constructor_pattern = constructor::get_constructor_pattern(
                        tokenizer,
                        token.to_string().as_str(),
                    )?;
                    go(
                        tokenizer,
                        prev_expression.apply_with(Expr::Constructor0(constructor_pattern)),
                    )
                }

                Token::MultiChar(MultiCharTokens::Worker) => {
                    go(tokenizer, prev_expression.apply_with(Expr::Worker()))
                }

                Token::MultiChar(MultiCharTokens::InterpolationStart) => {
                    let new_expr = capture_expression_until(
                        tokenizer,
                        vec![&Token::interpolation_start(), &Token::LCurly],
                        Some(&Token::RCurly),
                        prev_expression,
                        go,
                    )?;

                    go(tokenizer, new_expr)
                }

                Token::Quote => {
                    let new_expr = quoted::create_expr_between_quotes(tokenizer)?;
                    go(tokenizer, prev_expression.apply_with(new_expr))
                }

                Token::LParen => tuple::create_tuple(tokenizer)
                    .and_then(|tuple| go(tokenizer, prev_expression.apply_with(tuple))),

                Token::MultiChar(MultiCharTokens::GreaterThanOrEqualTo) => {
                    if prev_expression.is_empty() {
                        return Err(ParseError::Message(
                            "GreaterThanOrEqualTo (>=) is applied to a non existing left expression"
                                .to_string(),
                        ));
                    };

                    let new_expr = build_with_last_complete_expr(
                        InCompleteExpressionContext::GreaterThanOrEqualTo,
                        prev_expression,
                        |prev, new| {
                            InternalExprResult::complete(Expr::GreaterThanOrEqualTo(
                                Box::new(prev),
                                Box::new(new),
                            ))
                        },
                    )?;

                    go(tokenizer, new_expr)
                }

                Token::GreaterThan => {
                    if prev_expression.is_empty() {
                        return Err(ParseError::Message(
                            "GreaterThan (>) is applied to a non existing left expression"
                                .to_string(),
                        ));
                    };

                    let new_expr = build_with_last_complete_expr(
                        InCompleteExpressionContext::GreaterThan,
                        prev_expression,
                        |prev, new| {
                            InternalExprResult::complete(Expr::GreaterThan(
                                Box::new(prev),
                                Box::new(new),
                            ))
                        },
                    )?;

                    go(tokenizer, new_expr)
                }

                Token::MultiChar(MultiCharTokens::Let) => {
                    let let_expr = let_statement::create_let_statement(tokenizer)?;
                    go(tokenizer, prev_expression.accumulate_with(let_expr))
                }

                Token::LessThan => {
                    if prev_expression.is_empty() {
                        return Err(ParseError::Message(
                            "LessThan (<) is applied to a non existing left expression".to_string(),
                        ));
                    };

                    let new_expr = build_with_last_complete_expr(
                        InCompleteExpressionContext::LessThan,
                        prev_expression,
                        |prev, new| {
                            InternalExprResult::complete(Expr::LessThan(
                                Box::new(prev),
                                Box::new(new),
                            ))
                        },
                    )?;

                    go(tokenizer, new_expr)
                }

                Token::MultiChar(MultiCharTokens::LessThanOrEqualTo) => {
                    if prev_expression.is_empty() {
                        return Err(ParseError::Message(
                            "LessThanOrEqualTo (<=)  is applied to a non existing left expression"
                                .to_string(),
                        ));
                    };

                    let new_expr = build_with_last_complete_expr(
                        InCompleteExpressionContext::LessThanOrEqualTo,
                        prev_expression,
                        |prev, new| {
                            InternalExprResult::complete(Expr::LessThanOrEqualTo(
                                Box::new(prev),
                                Box::new(new),
                            ))
                        },
                    )?;

                    go(tokenizer, new_expr)
                }

                Token::MultiChar(MultiCharTokens::EqualTo) => {
                    if prev_expression.is_empty() {
                        return Err(ParseError::Message(
                            "EqualTo (=) is applied to a non existing left expression".to_string(),
                        ));
                    };

                    let new_expr = build_with_last_complete_expr(
                        InCompleteExpressionContext::EqualTo,
                        prev_expression,
                        |prev, new| {
                            InternalExprResult::complete(Expr::EqualTo(
                                Box::new(prev),
                                Box::new(new),
                            ))
                        },
                    )?;

                    go(tokenizer, new_expr)
                }

                Token::Dot => match prev_expression {
                    InternalExprResult::Complete(expr) => {
                        let expr = selection::get_select_field(tokenizer, expr)?;
                        go(tokenizer, InternalExprResult::complete(expr))
                    }

                    _ => Err(ParseError::message(
                        "Invalid token field. Make sure expression format is correct",
                    )),
                },

                Token::LSquare => match prev_expression {
                    InternalExprResult::Complete(prev_expr) => {
                        let new_expr = selection::get_select_index(tokenizer, &prev_expr)?;
                        go(tokenizer, InternalExprResult::complete(new_expr))
                    }
                    _ => {
                        let expr = sequence::create_sequence(tokenizer)?;
                        go(tokenizer, prev_expression.apply_with(expr))
                    }
                },

                Token::MultiChar(MultiCharTokens::If) => {
                    let if_expr = if_condition::create_if_condition(tokenizer)?;

                    dbg!(&if_expr);

                    let res = go(tokenizer, prev_expression.apply_with(if_expr))?;
                    dbg!(&res);
                    Ok(res)
                }

                Token::MultiChar(MultiCharTokens::Match) => {
                    let new_expr = pattern_match::create_pattern_match_expr(tokenizer)?;

                    go(tokenizer, InternalExprResult::complete(new_expr))
                }

                Token::MultiChar(MultiCharTokens::Then) => match prev_expression {
                    InternalExprResult::InComplete(InCompleteExpressionContext::Then, _) => {
                        let mew_expr = capture_expression_until(
                            tokenizer,
                            vec![&Token::then()],
                            Some(&Token::else_token()),
                            prev_expression,
                            go,
                        )?;

                        go(tokenizer, mew_expr)
                    }

                    _ => Err(ParseError::Message(
                        "then is a keyword and should be part of a if condition logic".to_string(),
                    )),
                },

                Token::MultiChar(MultiCharTokens::Else) => match prev_expression {
                    InternalExprResult::InComplete(InCompleteExpressionContext::Else, _) => {
                        let expr = capture_expression_until(
                            tokenizer,
                            vec![&Token::else_token()],
                            None,
                            prev_expression,
                            go,
                        )?;

                        go(tokenizer, expr)
                    }
                    _ => Err(ParseError::Message(
                        "else is a keyword and should be part of a if-then condition logic"
                            .to_string(),
                    )),
                },
                Token::LCurly => {
                    let expr = if flags::is_flags(tokenizer) {
                        flags::create_flags(tokenizer)
                    } else {
                        record::create_record(tokenizer)
                    };

                    go(tokenizer, prev_expression.apply_with(expr?))
                }

                Token::RCurly => go(tokenizer, prev_expression),
                Token::RSquare => go(tokenizer, prev_expression),
                Token::RParen => go(tokenizer, prev_expression),
                Token::Space => go(tokenizer, prev_expression),
                Token::NewLine => go(tokenizer, prev_expression),
                Token::LetEqual => go(tokenizer, prev_expression),
                Token::SemiColon => go(tokenizer, prev_expression),
                Token::MultiChar(MultiCharTokens::Arrow) => go(tokenizer, prev_expression),
                Token::Comma => go(tokenizer, prev_expression),
                Token::Colon => go(tokenizer, prev_expression),
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

    let mut tokenizer: Tokenizer =  Tokenizer::new(input.as_ref());

    go(&mut tokenizer, InternalExprResult::Empty)
}

mod internal {
    use crate::expression::Expr;
    use crate::parser::expr::{constructor, util};
    use crate::parser::ParseError;
    use crate::tokeniser::tokenizer::{Token, Tokenizer};
    use strum_macros::Display;

    // While at every node (Token), we can somehow form a complete expression by peeking ahead using tokenizer multiple times,
    // we can avoid this at times, and form an in-complete expression using `InternalExprResult`.
    // This allows us to not worry about future at every token node.
    // Example: At Token::GreaterThan, we simply form an incomplete expression with prev exression (Ex: incomplete_expr  `1 >`),
    // which will get completed in further loop (Ex: complete_expr `1 > 2`)
    // Another example is Token::If, where we parse only predicate and leave it to the rest of the parsing to get the branches.
    // If we have a concrete idea of what the future should be, then it's good idea to peak ahead and complete and avoid building function.
    // Example: We really need all the tokens until the next curly `}` in a pattern match. So at Token::Match, we peak ahead many times and get a
    // complete match expression.
    pub(crate) enum InternalExprResult {
        Complete(Expr),
        InComplete(
            InCompleteExpressionContext,
            Box<dyn Fn(Expr) -> InternalExprResult>,
        ),
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

        pub(crate) fn accumulate_with(&self, expr: Expr) -> InternalExprResult {
            match self {
                InternalExprResult::Complete(complete_expr) => match complete_expr {
                    Expr::Multiple(vec) => {
                        let mut new_expr = vec.clone();
                        new_expr.push(expr);
                        InternalExprResult::complete(Expr::Multiple(new_expr))
                    }
                    _ => InternalExprResult::complete(Expr::Multiple(vec![
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

        pub(crate) fn incomplete<F>(scope: InCompleteExpressionContext, f: F) -> InternalExprResult
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
    #[derive(Display, Debug)]
    pub(crate) enum InCompleteExpressionContext {
        If,
        Else,
        Then,
        LessThan,
        GreaterThan,
        EqualTo,
        GreaterThanOrEqualTo,
        LessThanOrEqualTo,
    }

    // Returns a custom constructor if the string is followed by paranthesis
    pub(crate) fn get_expr_from_custom_string(
        tokenizer: &mut Tokenizer,
        custom_string: &str,
    ) -> Result<Expr, ParseError> {
        let next_token = tokenizer.peek_next_token();

        match next_token {
            Some(Token::LParen) => {
                let constructor_pattern =
                    constructor::get_constructor_pattern(tokenizer, custom_string)?;
                Ok(Expr::Constructor0(constructor_pattern))
            }
            _ => Ok(util::get_primitive_expr(custom_string)),
        }
    }

    // possible_nested_token_starts
    // corresponds to the tokens whose closed end is same as capture_until
    // and we should include those capture_untils
    pub(crate) fn capture_expression_until<F>(
        tokenizer: &mut Tokenizer,
        _possible_nested_token_starts: Vec<&Token>,
        capture_until: Option<&Token>,
        future_expression: InternalExprResult,
        get_expr: F,
    ) -> Result<InternalExprResult, ParseError>
    where
        F: FnOnce(&mut Tokenizer, InternalExprResult) -> Result<Expr, ParseError>,
    {
        let optional_captured_string = match capture_until {
            Some(last_token) => tokenizer.capture_string_until(last_token),
            None => tokenizer.capture_tail(),
        };

        match optional_captured_string {
            Some(captured_string) => {
                let mut new_tokenizer = Tokenizer::new(captured_string.as_str());

                let inner_expr = get_expr(&mut new_tokenizer, InternalExprResult::Empty)?;

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
        scope: InCompleteExpressionContext,
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
    use crate::expression::{ConstructorPattern, PatternMatchArm};

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
            .parse("${if request.path.user_id == 1 then 1 else if request.path.user_id == 2 then 2 else if request.path.user_id == 3 then 3 else 0}")
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

    //  We ignore this test as we stopped supporting using ( for nestedness
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
            .parse("foo-${if (if request.path.hello then 1 else 0) > 0) then request.path.user_id else 0}")
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
                PatternMatchArm((
                    ConstructorPattern::constructor(
                        "some",
                        vec![ConstructorPattern::Literal(Box::new(Expr::Variable(
                            "foo".to_string(),
                        )))],
                    )
                    .unwrap(),
                    Box::new(Expr::Variable("foo".to_string())),
                )),
                PatternMatchArm((
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
                PatternMatchArm((
                    ConstructorPattern::constructor(
                        "ok",
                        vec![ConstructorPattern::Literal(Box::new(Expr::Variable(
                            "foo".to_string(),
                        )))],
                    )
                    .unwrap(),
                    Box::new(Expr::Variable("foo".to_string())),
                )),
                PatternMatchArm((
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
                PatternMatchArm((
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
                PatternMatchArm((
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
                PatternMatchArm((
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
                PatternMatchArm((
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
                PatternMatchArm((
                    ConstructorPattern::constructor(
                        "some",
                        vec![ConstructorPattern::Literal(Box::new(Expr::Variable(
                            "foo".to_string(),
                        )))],
                    )
                    .unwrap(),
                    Box::new(Expr::Literal("foo".to_string())),
                )),
                PatternMatchArm((
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

    #[test]
    fn test_match_with_if_else() {
        let expression_parser = ExprParser {};

        let result = expression_parser
            .parse(
                "${match worker.response { some(foo) => if foo > 1 then foo else 0, none => 0 } }",
            )
            .unwrap();

        let expected = Expr::PatternMatch(
            Box::new(Expr::SelectField(
                Box::new(Expr::Worker()),
                "response".to_string(),
            )),
            vec![
                PatternMatchArm((
                    ConstructorPattern::constructor(
                        "some",
                        vec![ConstructorPattern::Literal(Box::new(Expr::Variable(
                            "foo".to_string(),
                        )))],
                    )
                    .unwrap(),
                    Box::new(Expr::Cond(
                        Box::new(Expr::GreaterThan(
                            Box::new(Expr::Variable("foo".to_string())),
                            Box::new(Expr::unsigned_integer(1)),
                        )),
                        Box::new(Expr::Variable("foo".to_string())),
                        Box::new(Expr::unsigned_integer(0)),
                    )),
                )),
                PatternMatchArm((
                    ConstructorPattern::constructor("none", vec![]).unwrap(),
                    Box::new(Expr::unsigned_integer(0)),
                )),
            ],
        );

        assert_eq!(result, expected);
    }

    #[test]
    fn test_match_with_if_else_with_record() {
        let expression_parser = ExprParser {};

        let result = expression_parser
            .parse("${match worker.response { some(foo) => if foo > 1 then { a : 'foo' } else 0, none => { a : 'bar' } } }")
            .unwrap();

        let expected = Expr::PatternMatch(
            Box::new(Expr::SelectField(
                Box::new(Expr::Worker()),
                "response".to_string(),
            )),
            vec![
                PatternMatchArm((
                    ConstructorPattern::constructor(
                        "some",
                        vec![ConstructorPattern::Literal(Box::new(Expr::Variable(
                            "foo".to_string(),
                        )))],
                    )
                    .unwrap(),
                    Box::new(Expr::Cond(
                        Box::new(Expr::GreaterThan(
                            Box::new(Expr::Variable("foo".to_string())),
                            Box::new(Expr::unsigned_integer(1)),
                        )),
                        Box::new(Expr::Record(vec![(
                            "a".to_string(),
                            Box::new(Expr::Literal("foo".to_string())),
                        )])),
                        Box::new(Expr::unsigned_integer(0)),
                    )),
                )),
                PatternMatchArm((
                    ConstructorPattern::constructor("none", vec![]).unwrap(),
                    Box::new(Expr::Record(vec![(
                        "a".to_string(),
                        Box::new(Expr::Literal("bar".to_string())),
                    )])),
                )),
            ],
        );

        assert_eq!(result, expected);
    }
}
