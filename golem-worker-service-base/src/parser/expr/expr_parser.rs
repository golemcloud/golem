use crate::expression::Expr;
use crate::tokeniser::tokenizer::{MultiCharTokens, Token, Tokenizer};

use crate::parser::expr::{
    code_block, constructor, flags, if_condition, let_statement, math_op, params, pattern_match,
    record, selection, sequence, tuple, util,
};
use crate::parser::{GolemParser, ParseError};
use internal::*;

#[derive(Clone, Debug)]
pub struct ExprParser {}

impl GolemParser<Expr> for ExprParser {
    fn parse(&self, input: &str) -> Result<Expr, ParseError> {
        parse_text(input)
    }
}

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
    let mut multi_line_expressions: ExpressionBuilder = ExpressionBuilder::default();
    let mut previous_expression: ConcatenatedExpr = ConcatenatedExpr::default();

    let mut tokenizer: Tokenizer = Tokenizer::new(input.as_ref());

    while let Some(token) = tokenizer.next_non_empty_token() {
        match token {
            Token::MultiChar(MultiCharTokens::Identifier(var)) => {
                previous_expression.build(Expr::Identifier(var))
            }

            Token::MultiChar(MultiCharTokens::StringLiteral(raw_string)) => {
                // we are parsing text (in turn calls parse code) with mutual recursion to handle
                // string interpolations
                let text = parse_text(raw_string.as_str())?;
                previous_expression.build(text);
            }

            Token::MultiChar(MultiCharTokens::NumberLiteral(number)) => {
                let new_expr = util::get_primitive_expr(number.as_str());
                previous_expression.build(new_expr);
            }

            Token::MultiChar(MultiCharTokens::BooleanLiteral(boolean)) => {
                let new_expr = boolean.parse::<bool>().map(Expr::Boolean).map_err(|_| {
                    ParseError::Message(format!("Invalid boolean literal {}", boolean))
                })?;
                previous_expression.build(new_expr);
            }

            token @ Token::MultiChar(MultiCharTokens::Some)
            | token @ Token::MultiChar(MultiCharTokens::None)
            | token @ Token::MultiChar(MultiCharTokens::Ok)
            | token @ Token::MultiChar(MultiCharTokens::Err) => {
                let expr =
                    constructor::create_constructor(&mut tokenizer, token.to_string().as_str())?;
                previous_expression.build(expr);
            }

            Token::MultiChar(MultiCharTokens::InterpolationStart) => {
                let code_block = code_block::create_code_block(&mut tokenizer)?;
                previous_expression.build(code_block);
            }

            Token::Quote => {}

            Token::LParen => {
                if let Some(concatenated_str) = previous_expression.get_concatenated_str()? {
                    if let Some(param_str) =
                        tokenizer.capture_string_until_and_skip_end(&Token::RParen)
                    {
                        let params = params::get_params(param_str.as_str())?;
                        previous_expression.reset_and_build(Expr::Call(concatenated_str, params));
                    } else {
                        previous_expression.reset_and_build(Expr::Call(concatenated_str, vec![]));
                    }
                } else {
                    previous_expression.build(tuple::create_tuple(&mut tokenizer)?)
                }
            }

            Token::MultiChar(MultiCharTokens::GreaterThanOrEqualTo) => {
                let left_op = previous_expression
                    .clone()
                    .final_expr()?
                    .ok_or::<ParseError>(
                        "GreaterThanOrEqualTo (>=) is applied to a non existing left expression"
                            .into(),
                    )?;

                let new_expr =
                    math_op::create_binary_op(&mut tokenizer, left_op, |left, right| {
                        Expr::GreaterThanOrEqualTo(left, right)
                    })?;

                previous_expression.reset_and_build(new_expr);
            }

            Token::GreaterThan => {
                let left_op = previous_expression
                    .clone()
                    .final_expr()?
                    .ok_or::<ParseError>(
                        "GreaterThan (>) is applied to a non existing left expression".into(),
                    )?;

                let new_expr =
                    math_op::create_binary_op(&mut tokenizer, left_op, |left, right| {
                        Expr::GreaterThan(left, right)
                    })?;

                previous_expression.reset_and_build(new_expr);
            }

            Token::LessThan => {
                let left_op = previous_expression
                    .clone()
                    .final_expr()?
                    .ok_or::<ParseError>(
                        "LessThan (<) is applied to a non existing left expression".into(),
                    )?;

                let new_expr =
                    math_op::create_binary_op(&mut tokenizer, left_op, |left, right| {
                        Expr::LessThan(left, right)
                    })?;

                previous_expression.reset_and_build(new_expr);
            }

            Token::MultiChar(MultiCharTokens::LessThanOrEqualTo) => {
                let left_op = previous_expression
                    .clone()
                    .final_expr()?
                    .ok_or::<ParseError>(
                        "LessThanOrEqualTo (<=) is applied to a non existing left expression"
                            .into(),
                    )?;

                let new_expr =
                    math_op::create_binary_op(&mut tokenizer, left_op, |left, right| {
                        Expr::LessThanOrEqualTo(left, right)
                    })?;

                previous_expression.reset_and_build(new_expr);
            }

            Token::MultiChar(MultiCharTokens::EqualTo) => {
                let left_op = previous_expression
                    .clone()
                    .final_expr()?
                    .ok_or::<ParseError>(
                        "EqualTo (==) is applied to a non existing left expression".into(),
                    )?;

                let new_expr =
                    math_op::create_binary_op(&mut tokenizer, left_op, |left, right| {
                        Expr::EqualTo(left, right)
                    })?;

                previous_expression.reset_and_build(new_expr);
            }

            Token::MultiChar(MultiCharTokens::Let) => {
                let let_expr = let_statement::create_let_statement(&mut tokenizer)?;
                previous_expression.reset_and_build(let_expr);
            }

            Token::Dot => {
                let expr = previous_expression
                    .clone()
                    .final_expr()
                    .or_else(|_| {
                        previous_expression
                            .get_concatenated_str()
                            .map(|opt| opt.map(|str| Expr::Literal(str)))
                    })?
                    .ok_or::<ParseError>(
                        "Selection of field is applied to a non existing left expression".into(),
                    )?;

                let expr = selection::get_select_field(&mut tokenizer, expr)?;
                previous_expression.reset_and_build(expr);
            }

            Token::LSquare => {
                if let Some(expr) = previous_expression.final_expr()? {
                    let expr = selection::get_select_index(&mut tokenizer, &expr)?;
                    previous_expression.reset_and_build(expr);
                } else {
                    let expr = sequence::create_sequence(&mut tokenizer)?;
                    previous_expression.build(expr);
                }
            }

            Token::MultiChar(MultiCharTokens::If) => {
                let if_expr = if_condition::create_if_condition(&mut tokenizer)?;
                previous_expression.build(if_expr);
            }

            Token::MultiChar(MultiCharTokens::Match) => {
                let new_expr = pattern_match::create_pattern_match_expr(&mut tokenizer)?;

                previous_expression.build(new_expr);
            }

            Token::MultiChar(MultiCharTokens::Then) => {
                return Err(ParseError::Message(
                    "then is a keyword and should be part of a if condition logic".to_string(),
                ));
            }

            Token::MultiChar(MultiCharTokens::Else) => {
                return Err(ParseError::Message(
                    "else is a keyword and should be part of a if condition logic".to_string(),
                ));
            }

            Token::SemiColon => {
                if let Some(expr) = &previous_expression.final_expr()? {
                    multi_line_expressions.build(expr.clone());
                }

                previous_expression = ConcatenatedExpr::default();
            }

            Token::LCurly => {
                let expr = if flags::is_flags(&mut tokenizer) {
                    flags::create_flags(&mut tokenizer)
                } else {
                    record::create_record(&mut tokenizer)
                }?;

                previous_expression.build(expr);
            }
            Token::WildCard => {
                return Err(
                    format!("Wild card at {} is not a valid expression", tokenizer.pos()).into(),
                )
            }
            Token::At => {
                return Err(format!("@ at {} is not a valid expression", tokenizer.pos()).into())
            }

            Token::MultiChar(MultiCharTokens::Arrow) => {
                return Err(
                    format!("Arrow at {} is not a valid expression", tokenizer.pos()).into(),
                )
            }
            Token::Escape => {}
            Token::RCurly => {}
            Token::RSquare => {}
            Token::RParen => {}
            Token::Space => {}
            Token::NewLine => {}
            Token::LetEqual => {}
            Token::Comma => {}
            Token::Colon => previous_expression.build(Expr::Literal(":".to_string())),
        }
    }

    if let Some(prev_expr) = previous_expression.final_expr()? {
        multi_line_expressions.build(prev_expr);
    }

    Ok(multi_line_expressions.get_and_reset())
}

mod internal {
    use crate::expression;
    use crate::expression::Expr;
    use crate::parser::ParseError;

    #[derive(Clone, Default)]
    pub(crate) struct ConcatenatedExpr {
        expressions: Vec<Expr>,
    }

    impl ConcatenatedExpr {
        pub(crate) fn is_empty(&self) -> bool {
            self.expressions.is_empty()
        }

        pub(crate) fn build(&mut self, expr: Expr) {
            self.expressions.push(expr)
        }

        pub(crate) fn reset_and_build(&mut self, expr: Expr) {
            self.expressions = vec![expr]
        }

        pub(crate) fn final_expr(&self) -> Result<Option<Expr>, ParseError> {
            if self.expressions.len() > 1 {
                Err(ParseError::Message(
                    "Expressions to be separated by semi column".to_string(),
                ))
            } else {
                let first = self.expressions.first();
                Ok(first.cloned())
            }
        }

        pub(crate) fn get_concatenated_str(&self) -> Result<Option<String>, ParseError> {
            if self.is_empty() {
                Ok(None)
            } else {
                let mut vec = vec![];
                for i in &self.expressions {
                    let str = match i {
                        Expr::Identifier(str) => Ok(str.to_string()),
                        Expr::Literal(str) => Ok(str.to_string()),
                        // TODO; https://www.notion.so/golemcloud/Rib-Supporting-New-Function-Syntax-ed92a3f1b92e4dd2a768402127f9aa7f
                        // This needs to be done better and is documented above
                        // The reasoning here is, we consider a previous expression to be a string only if they are actually string,
                        // but by the time we decided its an invocation of a function/variant call the possible name of the function or variant
                        // was parsed a selection-field, and revert it back to Str
                        Expr::SelectField(expr, field) => Ok(format!("{}.{}", expr, field)),
                        expr => Err(ParseError::Message(format!(
                            "Invalid expression: {}",
                            expression::to_string(expr).unwrap()
                        ))),
                    };

                    vec.push(str?)
                }

                Ok(Some(vec.join("")))
            }
        }
    }

    #[derive(Default)]
    pub(crate) struct ExpressionBuilder {
        expressions: Vec<Expr>,
    }

    impl ExpressionBuilder {
        pub(crate) fn build(&mut self, expr: Expr) {
            self.expressions.push(expr);
        }

        pub(crate) fn get_and_reset(&mut self) -> Expr {
            let expressions = std::mem::take(&mut self.expressions);

            if expressions.len() == 1 {
                expressions[0].clone()
            } else {
                Expr::Multiple(expressions)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expression::{ArmPattern, InnerNumber, MatchArm};

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
        let request = Expr::Identifier("request".to_string());
        let select_body = Expr::SelectField(Box::new(request), "body".to_string());

        let select_input = Expr::SelectField(Box::new(select_body), "input".to_string());

        let first_index = Expr::SelectIndex(Box::new(select_input), 0);

        assert_eq!(result, Ok(first_index));
    }

    #[test]
    fn expr_parser_with_worker_result_path() {
        let expression_parser = ExprParser {};

        let result = expression_parser.parse("${worker.response.input[0]}");
        let worker = Expr::Identifier("worker".to_string());
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
                    Box::new(Expr::Identifier("request".to_string())),
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
                    Box::new(Expr::Identifier("request".to_string())),
                    "path".to_string(),
                )),
                "user-id".to_string(),
            )),
            Box::new(Expr::SelectField(
                Box::new(Expr::SelectField(
                    Box::new(Expr::Identifier("request".to_string())),
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
                    Box::new(Expr::Identifier("request".to_string())),
                    "path".to_string(),
                )),
                "user-id".to_string(),
            )),
            Box::new(Expr::SelectField(
                Box::new(Expr::SelectField(
                    Box::new(Expr::Identifier("request".to_string())),
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
                    Box::new(Expr::Identifier("request".to_string())),
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
                    Box::new(Expr::Identifier("request".to_string())),
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
                Box::new(Expr::Identifier("request".to_string())),
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
            Box::new(Expr::Identifier("hello".to_string())),
            Box::new(Expr::Identifier("foo".to_string())),
            Box::new(Expr::Identifier("bar".to_string())),
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
            Box::new(Expr::Identifier("foo".to_string())),
            Box::new(Expr::unsigned_integer(1)),
            Box::new(Expr::Cond(
                Box::new(Expr::Identifier("bar".to_string())),
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
                        Box::new(Expr::Identifier("request".to_string())),
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
                            Box::new(Expr::Identifier("request".to_string())),
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
                                Box::new(Expr::Identifier("request".to_string())),
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
                    Box::new(Expr::Identifier("request".to_string())),
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
                    Box::new(Expr::Identifier("request".to_string())),
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
                    Box::new(Expr::Identifier("request".to_string())),
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
                    Box::new(Expr::Identifier("request".to_string())),
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
                    Box::new(Expr::Identifier("request".to_string())),
                    "path".to_string(),
                )),
                "user_id".to_string(),
            )),
            Box::new(Expr::SelectField(
                Box::new(Expr::SelectField(
                    Box::new(Expr::Identifier("request".to_string())),
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
                    Box::new(Expr::Identifier("request".to_string())),
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
                            Box::new(Expr::Identifier("request".to_string())),
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
                        Box::new(Expr::Identifier("request".to_string())),
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
                        Box::new(Expr::Identifier("request".to_string())),
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
                        Box::new(Expr::Identifier("request".to_string())),
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
                        Box::new(Expr::Identifier("request".to_string())),
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
                Box::new(Expr::Identifier("worker".to_string())),
                "response".to_string(),
            )),
            vec![
                MatchArm((
                    ArmPattern::from("some", vec![ArmPattern::from("foo", vec![]).unwrap()])
                        .unwrap(),
                    Box::new(Expr::Identifier("foo".to_string())),
                )),
                MatchArm((
                    ArmPattern::from("none", vec![]).unwrap(),
                    Box::new(Expr::Identifier("result2".to_string())),
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
                Box::new(Expr::Identifier("worker".to_string())),
                "response".to_string(),
            )),
            vec![
                MatchArm((
                    ArmPattern::from("ok", vec![ArmPattern::from("foo", vec![]).unwrap()]).unwrap(),
                    Box::new(Expr::Identifier("foo".to_string())),
                )),
                MatchArm((
                    ArmPattern::from("err", vec![ArmPattern::from("bar", vec![]).unwrap()])
                        .unwrap(),
                    Box::new(Expr::Identifier("result2".to_string())),
                )),
            ],
        );

        assert_eq!(result, expected);
    }

    #[test]
    fn test_if_expr_with_pattern_match_variable_and_constants() {
        let expression_parser = ExprParser {};

        let result = expression_parser
            .parse(
                r#"${match worker.response { some(foo) => worker.response, none => "nothing" } }"#,
            )
            .unwrap();

        let expected = Expr::PatternMatch(
            Box::new(Expr::SelectField(
                Box::new(Expr::Identifier("worker".to_string())),
                "response".to_string(),
            )),
            vec![
                MatchArm((
                    ArmPattern::from("some", vec![ArmPattern::from("foo", vec![]).unwrap()])
                        .unwrap(),
                    Box::new(Expr::SelectField(
                        Box::new(Expr::Identifier("worker".to_string())),
                        "response".to_string(),
                    )),
                )),
                MatchArm((
                    ArmPattern::from("none", vec![]).unwrap(),
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
            .parse(r#"${match worker.response { some(some(foo)) => worker.response, none => "nothing" } }"#)
            .unwrap();

        let expected = Expr::PatternMatch(
            Box::new(Expr::SelectField(
                Box::new(Expr::Identifier("worker".to_string())),
                "response".to_string(),
            )),
            vec![
                MatchArm((
                    ArmPattern::from(
                        "some",
                        vec![ArmPattern::from(
                            "some",
                            vec![ArmPattern::from("foo", vec![]).unwrap()],
                        )
                        .unwrap()],
                    )
                    .unwrap(),
                    Box::new(Expr::SelectField(
                        Box::new(Expr::Identifier("worker".to_string())),
                        "response".to_string(),
                    )),
                )),
                MatchArm((
                    ArmPattern::from("none", vec![]).unwrap(),
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
            .parse("${match worker.response { some(foo) => \"foo\", none => \"bar bar\" } }")
            .unwrap();

        let expected = Expr::PatternMatch(
            Box::new(Expr::SelectField(
                Box::new(Expr::Identifier("worker".to_string())),
                "response".to_string(),
            )),
            vec![
                MatchArm((
                    ArmPattern::from("some", vec![ArmPattern::from("foo", vec![]).unwrap()])
                        .unwrap(),
                    Box::new(Expr::Literal("foo".to_string())),
                )),
                MatchArm((
                    ArmPattern::from("none", vec![]).unwrap(),
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
                Box::new(Expr::Identifier("worker".to_string())),
                "response".to_string(),
            )),
            vec![
                MatchArm((
                    ArmPattern::from("some", vec![ArmPattern::from("foo", vec![]).unwrap()])
                        .unwrap(),
                    Box::new(Expr::Cond(
                        Box::new(Expr::GreaterThan(
                            Box::new(Expr::Identifier("foo".to_string())),
                            Box::new(Expr::unsigned_integer(1)),
                        )),
                        Box::new(Expr::Identifier("foo".to_string())),
                        Box::new(Expr::unsigned_integer(0)),
                    )),
                )),
                MatchArm((
                    ArmPattern::from("none", vec![]).unwrap(),
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
            .parse(r#"${match worker.response { some(foo) => if foo > 1 then { a : "foo" } else 0, none => { a : "bar" } } }"#)
            .unwrap();

        let expected = Expr::PatternMatch(
            Box::new(Expr::SelectField(
                Box::new(Expr::Identifier("worker".to_string())),
                "response".to_string(),
            )),
            vec![
                MatchArm((
                    ArmPattern::from("some", vec![ArmPattern::from("foo", vec![]).unwrap()])
                        .unwrap(),
                    Box::new(Expr::Cond(
                        Box::new(Expr::GreaterThan(
                            Box::new(Expr::Identifier("foo".to_string())),
                            Box::new(Expr::unsigned_integer(1)),
                        )),
                        Box::new(Expr::Record(vec![(
                            "a".to_string(),
                            Box::new(Expr::Literal("foo".to_string())),
                        )])),
                        Box::new(Expr::unsigned_integer(0)),
                    )),
                )),
                MatchArm((
                    ArmPattern::from("none", vec![]).unwrap(),
                    Box::new(Expr::Record(vec![(
                        "a".to_string(),
                        Box::new(Expr::Literal("bar".to_string())),
                    )])),
                )),
            ],
        );

        assert_eq!(result, expected);
    }

    #[test]
    fn test_let_binding() {
        let expression_parser = ExprParser {};

        let result = expression_parser.parse("${let x = 1;}").unwrap();

        let expected = Expr::Let("x".to_string(), Box::new(Expr::unsigned_integer(1)));

        assert_eq!(result, expected);
    }

    #[test]
    fn multi_line_expressions() {
        let expression_parser = ExprParser {};

        let program = r"
           let x = 1;
           let y = x > 1;
           let z = y < x;
           x
        ";

        let result = expression_parser
            .parse(format!("${{{}}}", program).as_str())
            .unwrap();

        let expected = Expr::Multiple(vec![
            Expr::Let("x".to_string(), Box::new(Expr::unsigned_integer(1))),
            Expr::Let(
                "y".to_string(),
                Box::new(Expr::GreaterThan(
                    Box::new(Expr::Identifier("x".to_string())),
                    Box::new(Expr::unsigned_integer(1)),
                )),
            ),
            Expr::Let(
                "z".to_string(),
                Box::new(Expr::LessThan(
                    Box::new(Expr::Identifier("y".to_string())),
                    Box::new(Expr::Identifier("x".to_string())),
                )),
            ),
            Expr::Identifier("x".to_string()),
        ]);

        assert_eq!(result, expected);
    }

    #[test]
    fn test_function_call1() {
        let expression_parser = ExprParser {};

        let result = expression_parser
            .parse("${api:abc/get-cart-contents(1, 2, request.user-id)}")
            .unwrap();

        let expected = Expr::Call(
            "api:abc/get-cart-contents".to_string(),
            vec![
                Expr::Number(InnerNumber::UnsignedInteger(1)),
                Expr::Number(InnerNumber::UnsignedInteger(2)),
                Expr::SelectField(
                    Box::new(Expr::Identifier("request".to_string())),
                    "user-id".to_string(),
                ),
            ],
        );

        assert_eq!(result, expected);
    }

    #[test]
    fn test_function_call_with_zero_params() {
        let expression_parser = ExprParser {};

        let result = expression_parser.parse("${foo()}").unwrap();

        let expected = Expr::Call("foo".to_string(), vec![]);

        assert_eq!(result, expected);
    }
}
