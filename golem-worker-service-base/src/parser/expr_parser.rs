use std::rc::Rc;

use crate::expression::{ConstructorPattern, Expr};
use crate::tokeniser::tokenizer::{MultiCharTokens, Token, Tokenizer};

use super::*;
use internal::*;

#[derive(Clone, Debug)]
pub struct ExprParser {}

// Expr parsing can be done within a context. If unsure what the context should be, select Context::Code
// Context allows us to handle things such as string-interpolation easily. Ex: if context is Text,
// `foo>1` will result in Expr::Concat("foo", ">" , "1")
// and `foo>${user-id}` will be Expr::Concat("foo", ">", Expr::Variable("user-id")). Evaluator will look for values for this variable.
// Had it Context::Code, it would have been `Expr::GreaterThan(Expr::Variable("foo"), Expr::Variable("user-id"))`
#[derive(Clone, Debug)]
enum Context {
    Code,
    Text,
}

impl Context {
    fn is_code(&self) -> bool {
        match self {
            Context::Code => true,
            Context::Text => false,
        }
    }
}

impl GolemParser<Expr> for ExprParser {
    fn parse(&self, input: &str) -> Result<Expr, ParseError> {
        parse_with_context(input, Context::Text)
    }
}


fn parse_with_context(input: &str, context: Context) -> Result<Expr, ParseError> {
    let mut tokenizer: Tokenizer = tokenise(input);

    parse_tokens(&mut tokenizer, context)
}

fn parse_tokens(tokenizer: &mut Tokenizer, context: Context) -> Result<Expr, ParseError> {
    fn go(
        tokenizer: &mut Tokenizer,
        context: Context,
        prev_expression: InternalExprResult,
    ) -> Result<Expr, ParseError> {
        let token = if context.is_code() {
            tokenizer.next_non_empty_token()
        } else {
            tokenizer.next_token().map(|t| t.as_raw_string_token())
        };

        if let Some(token) = token {
            match token {
                Token::MultiChar(MultiCharTokens::Other(raw_string)) => {
                    let new_expr = if context.is_code() {
                        resolve_literal_in_code_context(raw_string.as_str())
                    } else {
                        Expr::Literal(raw_string)
                    };

                    go(tokenizer, context, prev_expression.apply_with(new_expr))
                }

                Token::MultiChar(MultiCharTokens::Number(number)) => {
                    let new_expr = if context.is_code() {
                        resolve_literal_in_code_context(number.as_str())
                    } else {
                        Expr::Literal(number)
                    };

                    go(tokenizer, context, prev_expression.apply_with(new_expr))
                }

                Token::MultiChar(MultiCharTokens::Request) => go(
                    tokenizer,
                    context,
                    prev_expression.apply_with(Expr::Request()),
                ),

                Token::MultiChar(MultiCharTokens::None) => go(
                    tokenizer,
                    context,
                    prev_expression.apply_with(Expr::Constructor0(
                        ConstructorPattern::constructor("none", vec![])?,
                    )),
                ),

                token @ Token::MultiChar(MultiCharTokens::Some)
                | token @ Token::MultiChar(MultiCharTokens::Ok)
                | token @ Token::MultiChar(MultiCharTokens::Err) => {
                    match tokenizer.next_non_empty_token() {
                        Some(Token::LParen) => {
                            let constructor_var_optional = tokenizer
                                .capture_string_until_and_skip_end(
                                    vec![&Token::LParen],
                                    &Token::RParen,
                                );
                            let constructor = match constructor_var_optional {
                                Some(constructor_var) => {
                                    let mut tokenizer = Tokenizer::new(constructor_var.as_str());

                                    let constructor_expr = go(
                                        &mut tokenizer,
                                        Context::Code,
                                        InternalExprResult::Empty,
                                    )?;

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
                                tokenizer,
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

                Token::MultiChar(MultiCharTokens::Worker) => go(
                    tokenizer,
                    context,
                    prev_expression.apply_with(Expr::Worker()),
                ),

                Token::MultiChar(MultiCharTokens::InterpolationStart) => {
                    let new_expr = capture_expression_until(
                        tokenizer,
                        vec![&Token::interpolation_start(), &Token::LCurly],
                        Some(&Token::RCurly),
                        prev_expression,
                        go,
                    )?;

                    go(tokenizer, context, new_expr)
                }

                Token::Quote => {
                    let non_code_string =
                        tokenizer.capture_string_until_and_skip_end(vec![], &Token::Quote);

                    let new_expr = match non_code_string {
                        Some(string) => {
                            let mut tokenizer = Tokenizer::new(string.as_str());

                            go(&mut tokenizer, Context::Text, InternalExprResult::Empty)
                        }
                        None => Err(ParseError::Message(
                            "Expecting a non-empty string between quotes".to_string(),
                        )),
                    };

                    go(tokenizer, context, prev_expression.apply_with(new_expr?))
                }

                Token::LParen => {
                    create_tuple(tokenizer).and_then(|tuple| {
                        go(tokenizer, context, prev_expression.apply_with(tuple))
                    })
                }

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

                    go(tokenizer, context, new_expr)
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

                    go(tokenizer, context, new_expr)
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

                    go(tokenizer, context, new_expr)
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

                    go(tokenizer, context, new_expr)
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

                    go(tokenizer, context, new_expr)
                }

                Token::Dot => {
                    // If a dot appears, then that means next token is probably a "field" selection rather than expression on its own
                    // and cannot delegate to further loops without peeking ahead using tokenizer and attaching the field to the current expression
                    let next_token = tokenizer.next_non_empty_token();

                    let possible_field = match next_token {
                        Some(Token::MultiChar(MultiCharTokens::Other(field))) => field,
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
                            tokenizer,
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

                Token::LSquare => match prev_expression {
                    InternalExprResult::Complete(prev_expr) => {
                        let optional_possible_index =
                            tokenizer.capture_string_until(vec![&Token::LSquare], &Token::RSquare);

                        match optional_possible_index {
                            Some(index) => {
                                if let Ok(index) = index.trim().parse::<usize>() {
                                    go(
                                        tokenizer,
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
                    _ => {
                        let expr = create_list(tokenizer)?;
                        go(tokenizer, context, prev_expression.apply_with(expr))
                    }
                },

                Token::MultiChar(MultiCharTokens::If) => {
                    // We expect to form Expr::Cond given three unknown variables
                    let future_expr = InternalExprResult::incomplete(
                        InCompleteExpressionContext::If,
                        move |first_result| {
                            let first_result: Rc<Expr> = Rc::new(first_result);
                            InternalExprResult::incomplete(
                                InCompleteExpressionContext::Then,
                                move |second_result| {
                                    let first_result: Rc<Expr> = Rc::clone(&first_result);
                                    InternalExprResult::incomplete(
                                        InCompleteExpressionContext::Else,
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
                        tokenizer,
                        vec![&Token::if_token()],
                        Some(&Token::then()),
                        future_expr,
                        go,
                    )?;

                    go(tokenizer, context, captured_predicate)
                }

                Token::MultiChar(MultiCharTokens::Match) => {
                    let expr_under_evaluation =
                        tokenizer.capture_string_until(vec![], &Token::LCurly);

                    let new_expr = match expr_under_evaluation {
                        Some(expr_under_evaluation) => {
                            let mut new_tokenizer = Tokenizer::new(expr_under_evaluation.as_str());
                            let expression =
                                go(&mut new_tokenizer, Context::Code, InternalExprResult::Empty)?;
                            match tokenizer.next_non_empty_token() {
                                Some(Token::LCurly) => {
                                    let constructors = get_constructors(tokenizer)?;
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

                    go(tokenizer, context, new_expr?)
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

                        go(tokenizer, context, mew_expr)
                    }

                    _  => Err(ParseError::Message(
                        "then is a keyword and should be part of a if condition logic"
                            .to_string(),
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

                        go(tokenizer, context, expr)
                    }
                    _ => Err(ParseError::Message(
                        "else is a keyword and should be part of a if-then condition logic"
                            .to_string(),
                    )),
                },

                Token::RCurly => go(tokenizer, context, prev_expression),
                Token::RSquare => go(tokenizer, context, prev_expression),
                Token::RParen => go(tokenizer, context, prev_expression),
                Token::Space => go(tokenizer, context, prev_expression),
                Token::NewLine => go(tokenizer, context, prev_expression),
                Token::LCurly => {
                    let expr = if is_flags(tokenizer)  {
                        create_flags(tokenizer)
                    } else {
                        create_record(
                            tokenizer,
                            vec![&Token::interpolation_start(), &Token::LCurly],
                            Some(&Token::RCurly),
                        )
                    };

                    go(tokenizer, context, prev_expression.apply_with(expr?))
                }
                Token::MultiChar(MultiCharTokens::Arrow) => go(tokenizer, context, prev_expression),
                Token::Comma => go(tokenizer, context, prev_expression),
                Token::Colon => go(tokenizer, context, prev_expression),
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

    go(tokenizer, context, InternalExprResult::Empty)
}

mod internal {
    use crate::expression::{ConstructorPattern, ConstructorPatternExpr, Expr, InnerNumber};
    use crate::parser::expr_parser::{parse_with_context, Context, parse_tokens};
    use crate::parser::ParseError;
    use crate::tokeniser::tokenizer::{MultiCharTokens, Token, Tokenizer};
    use strum_macros::Display;

    pub(crate) fn create_tuple(tokenizer: &mut Tokenizer) -> Result<Expr, ParseError> {
        let mut tuple_elements = vec![];
        let mut grouped_exprs: Vec<Expr> = vec![];

        fn go(tokenizer: &mut Tokenizer, tuple_elements: &mut Vec<Expr>, grouped_exprs: &mut Vec<Expr>) -> Result<(), ParseError> {
            let captured_string = tokenizer.capture_string_until(
                vec![],
                &Token::Comma, // Wave does this
            );

            match captured_string {
                Some(r) => {
                    let expr = parse_with_context(r.as_str(), Context::Code)?;

                    tuple_elements.push(expr);
                    tokenizer.next_non_empty_token(); // Skip Comma
                    go(tokenizer, tuple_elements, grouped_exprs)
                }

                None => {
                    let last_value =
                        tokenizer.capture_string_until_and_skip_end(vec![&Token::LParen], &Token::RParen);

                    match last_value {
                        Some(last_value) if !last_value.is_empty() => {
                            let expr = parse_with_context(last_value.as_str(), Context::Code)?;
                            // If there is only 1 element, and it's an invalid tuple element, then we need to push it to grouped_exprs
                            if tuple_elements.is_empty()  {
                                if !is_valid_tuple_element(&expr) {
                                    grouped_exprs.push(expr.clone());
                                }
                            }

                            tuple_elements.push(expr);

                            Ok(())
                        }
                        Some(_) => Ok(()),
                        None => Err(ParseError::Message("Expecting a closing paren (RParen)".to_string()))
                    }
                }
            }
        }

        go(tokenizer, &mut tuple_elements, &mut grouped_exprs)?;

        if !grouped_exprs.is_empty() {
            Ok(grouped_exprs.pop().unwrap())
        } else {
            Ok(Expr::Tuple(tuple_elements))
        }
    }

    // We allow only WIT types to be part of the tuple
    // and not expressions such as Expr::Cond, or pattern matching
    // However complex expressions can be contained within () to
    // allow grouping of expressions
    pub(crate) fn is_valid_tuple_element(expr: &Expr) -> bool {
        match expr {
            Expr::Variable(_) => true,
            Expr::Number(_) => true,
            Expr::Boolean(_) => true,
            Expr::Flags(_) => true,
            Expr::Record(_) => true,
            Expr::Sequence(_) => true,
            Expr::Concat(_) => true,
            Expr::Constructor0(_) => true,
            Expr::Tuple(_) => true,
            Expr::Literal(_) => true,
            Expr::Not(_) => false,
            Expr::Request() => false,
            Expr::Worker() => false,
            Expr::PathVar(_) => false,
            Expr::SelectField(_, _) => false,
            Expr::SelectIndex(_, _) => false,
            Expr::Cond(_, _, _) => false, // we disallow if statements within tuple
            Expr::PatternMatch(_, _) => false,
            Expr::GreaterThan(_, _) => false,
            Expr::LessThan(_, _) => false,
            Expr::EqualTo(_, _) => false,
            Expr::GreaterThanOrEqualTo(_, _) => false,
            Expr::LessThanOrEqualTo(_, _) => false,
        }
    }

    pub(crate) fn create_list(tokenizer: &mut Tokenizer) -> Result<Expr, ParseError> {
        let mut record = vec![];

        fn go(tokenizer: &mut Tokenizer, record: &mut Vec<Expr>) -> Result<(), ParseError> {
            let captured_string = tokenizer.capture_string_until(
                vec![],
                &Token::Comma, // Wave does this
            );

            match captured_string {
                Some(r) => {
                    let expr = parse_with_context(r.as_str(), Context::Code)?;

                    record.push(expr);
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

        go(tokenizer, &mut record)?;

        Ok(Expr::Sequence(record))
    }

    pub(crate) fn create_record(
        tokenizer: &mut Tokenizer,
        possible_nested_token_starts: Vec<&Token>,
        capture_until: Option<&Token>,
    ) -> Result<Expr, ParseError> {
        let mut record = vec![];

        fn go(
            tokenizer: &mut Tokenizer,
            record: &mut Vec<(String, Expr)>,
            possible_nested_token_starts: Vec<&Token>,
            capture_until: Option<&Token>,
        ) -> Result<(), ParseError> {
            // We already consumed first curly brace
            // Now we either expect a closing brace or a key
            // followed by a colon and then records being comma separated
            match tokenizer.next_non_empty_token() {
                Some(Token::RCurly) => Ok(()),
                Some(Token::MultiChar(MultiCharTokens::Other(key))) => {
                    let key = key.to_string();
                    let value = match tokenizer.next_non_empty_token() {
                        Some(Token::Colon) => {
                            let captured_string = tokenizer.capture_string_until(
                                possible_nested_token_starts.clone(),
                                capture_until.unwrap_or(&Token::Comma),
                            );

                            match captured_string {
                                Some(captured_string) => {
                                    let expr = parse_with_context(
                                        captured_string.as_str(),
                                        Context::Code,
                                    )?;
                                    Ok(expr)
                                }
                                None => Err(ParseError::Message(
                                    "Expecting a value after colon".to_string(),
                                )),
                            }
                        }
                        _ => Err(ParseError::Message(
                            "Expecting a colon after key in record".to_string(),
                        )),
                    };

                    match value {
                        Ok(expr) => {
                            record.push((key, expr));
                            go(
                                tokenizer,
                                record,
                                possible_nested_token_starts,
                                capture_until,
                            )
                        }
                        Err(e) => Err(e),
                    }
                }
                Some(token) => Err(ParseError::Message(format!(
                    "Expecting a key in record. But found {}",
                    token
                ))),
                None => Err(ParseError::Message(
                    "Expecting a key in record. But found nothing".to_string(),
                )),
            }
        }

        go(
            tokenizer,
            &mut record,
            possible_nested_token_starts,
            capture_until,
        )?;

        Ok(Expr::Record(
            record
                .iter()
                .map(|(s, e)| (s.clone(), Box::new(e.clone())))
                .collect::<Vec<_>>(),
        ))
    }

    pub(crate) fn is_flags(tokenizer: &mut Tokenizer) -> bool {
        let colon_index = tokenizer.index_of_future_token(vec![], &Token::Colon);
        let comma_index = tokenizer.index_of_future_token(vec![], &Token::Comma);
        match (comma_index, colon_index) {
            (Some(comma_index), Some(colon_index)) => comma_index < colon_index, // Comma exists before colon
            (None, Some(_)) => false, // Colon exists but no commas, meaning it can be record
            (Some(_), None) => true, // Comma exists but no colons, meaning its not a record
            (None, None) => true // No commas, no colons, but just strings between indicate flags
        }
    }

    pub(crate) fn create_flags(tokenizer: &mut Tokenizer) -> Result<Expr, ParseError> {
        fn go(
            tokenizer: &mut Tokenizer,
            flags: &mut Vec<String>
        ) -> Result<(), ParseError> {
            // We already skipped the first curly brace
            // We expect either a flag or curly brace, else fail
            match tokenizer.next_non_empty_token() {
                Some(Token::RCurly) => Ok(()), // Nothing more to do
                Some(Token::MultiChar(MultiCharTokens::Other(next_str))) => {
                    flags.push(next_str);
                    // If comma exists again, go again!
                    match tokenizer.next_non_empty_token() {
                        Some(Token::Comma) => {
                            go(tokenizer, flags)
                        }
                        // Otherwise it has to be curly brace, and this consumes all flags
                        Some(Token::RCurly) => Ok(()),
                        _ => Err(ParseError::Message(
                            "Expecting a comma or closing curly brace".to_string(),
                        )),
                    }
                }
                _ => Err(ParseError::Message(
                    "Expecting a flag or closing curly brace".to_string(),
                )),
            }
        }
        let mut flags = vec![];
        go(tokenizer, &mut flags)?;
        Ok(Expr::Flags(flags))
    }

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

    pub(crate) fn tokenise(input: &str) -> Tokenizer {
        Tokenizer::new(input)
    }

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
        InComplete(InCompleteExpressionContext, Box<dyn Fn(Expr) -> InternalExprResult>),
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

    pub(crate) fn get_constructors(
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

                            let constructor_var_optional = tokenizer
                                .capture_string_until_and_skip_end(
                                    vec![&Token::LParen],
                                    &Token::RParen,
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

    pub(crate) fn accumulate_constructor_pattern_expr<F>(
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
                let index_of_closed_curly_brace = tokenizer.index_of_future_token(
                    vec![&Token::LCurly, &Token::interpolation_start()],
                    &Token::RCurly,
                );
                let index_of_commaseparator =
                    tokenizer.index_of_future_token(vec![], &Token::Comma);

                match (index_of_closed_curly_brace, index_of_commaseparator) {
                    (Some(end_of_constructors), Some(comma)) => {
                        if end_of_constructors > comma {
                            let captured_string =
                                tokenizer.capture_string_until(vec![], &Token::Comma);

                            let individual_expr = parse_with_context(
                                captured_string.unwrap().as_str(),
                                Context::Code,
                            )
                            .map(|expr| {
                                ConstructorPatternExpr((constructor_pattern, Box::new(expr)))
                            })?;
                            collected_exprs.push(individual_expr);
                            tokenizer.next_non_empty_token(); // Skip CommaSeparator
                            accumulator(tokenizer, collected_exprs)
                        } else {
                            // End of constructor
                            let captured_string = tokenizer
                                .capture_string_until(vec![&Token::LCurly], &Token::RCurly);
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

                    // Last constructor
                    (Some(_), None) => {
                        let captured_string = tokenizer.capture_string_until(
                            vec![&Token::LCurly, &Token::interpolation_start()],
                            &Token::RCurly,
                        );

                        if let Some(captured_string) = captured_string {
                            let individual_expr = parse_with_context(
                                captured_string.as_str(),
                                Context::Code,
                            )
                            .map(|expr| {
                                ConstructorPatternExpr((constructor_pattern, Box::new(expr)))
                            })?;
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
    // possible_nested_token_starts
    // corresponds to the tokens whose closed end is same as capture_until
    // and we should include those capture_untils
    pub(crate) fn capture_expression_until<F>(
        tokenizer: &mut Tokenizer,
        possible_nested_token_starts: Vec<&Token>,
        capture_until: Option<&Token>,
        future_expression: InternalExprResult,
        get_expr: F,
    ) -> Result<InternalExprResult, ParseError>
    where
        F: FnOnce(&mut Tokenizer, Context, InternalExprResult) -> Result<Expr, ParseError>,
    {
        let optional_captured_string = match capture_until {
            Some(last_token) => {
                tokenizer.capture_string_until(possible_nested_token_starts, last_token)
            }
            None => tokenizer.capture_tail(),
        };

        match optional_captured_string {
            Some(captured_string) => {
                let mut new_tokenizer = Tokenizer::new(captured_string.as_str());

                let inner_expr =
                    get_expr(&mut new_tokenizer, Context::Code, InternalExprResult::Empty)?;

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
