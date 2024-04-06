use crate::expression::writer::WriterError;
use crate::expression::{writer, Expr};
use crate::parser::expr_parser::ExprParser;
use crate::parser::GolemParser;
use crate::parser::ParseError;

pub fn from_string(input: impl AsRef<str>) -> Result<Expr, ParseError> {
    let expr_parser = ExprParser {};
    expr_parser.parse(input.as_ref())
}

pub fn to_string(expr: &Expr) -> Result<String, WriterError> {
    writer::write_expr(expr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::expression::{Expr, InnerNumber};

    #[test]
    fn test_round_trip_read_write_literal() {
        let input_expr = Expr::Literal("hello".to_string());
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "hello".to_string();
        let output_expr = from_string(expr_str.clone()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_request() {
        let input_expr = Expr::Request();
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${request}".to_string();
        let output_expr = from_string(expr_str.clone()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_let() {
        let input_expr = Expr::Let(
            "x".to_string(),
            Box::new(Expr::Literal("hello".to_string())),
        );
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${let x = 'hello';}".to_string();
        let output_expr = from_string(expr_str.clone()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_worker() {
        let input_expr = Expr::Worker();
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${worker}".to_string();
        let output_expr = from_string(expr_str.clone()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_select_field() {
        let input_expr = Expr::SelectField(Box::new(Expr::Request()), "field".to_string());
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${request.field}".to_string();
        let output_expr = from_string(expr_str.clone()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_select_index() {
        let input_expr = Expr::SelectIndex(Box::new(Expr::Request()), 1);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${request[1]}".to_string();
        let output_expr = from_string(expr_str.clone()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_sequence() {
        let input_expr = Expr::Sequence(vec![Expr::Request(), Expr::Request()]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${[request, request]}".to_string();
        let output_expr = from_string(expr_str.clone()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_record() {
        let input_expr = Expr::Record(vec![
            ("field".to_string(), Box::new(Expr::Request())),
            ("field".to_string(), Box::new(Expr::Request())),
            ("field".to_string(), Box::new(Expr::Request())),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${{field: request, field: request, field: request}}".to_string();
        let output_expr = from_string(expr_str.clone()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_nested_record() {
        let input_expr = Expr::Record(vec![
            (
                "a".to_string(),
                Box::new(Expr::Record(vec![
                    ("ab".to_string(), Box::new(Expr::Request())),
                    ("ac".to_string(), Box::new(Expr::Request())),
                ])),
            ),
            (
                "b".to_string(),
                Box::new(Expr::Sequence(vec![Expr::Record(vec![
                    ("bc".to_string(), Box::new(Expr::Request())),
                    ("bd".to_string(), Box::new(Expr::Request())),
                ])])),
            ),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        dbg!(expr_str.clone());
        let record_string =
            "{a: {ab: request, ac: request}, b: {bc: request, bd: request}}".to_string();
        let expected_record_str = format!("${{{}}}", record_string); // Just wrapping it with interpolation
        let output_expr = from_string(expr_str.clone()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_record_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_tuple() {
        let input_expr = Expr::Tuple(vec![Expr::Request(), Expr::Request(), Expr::Request()]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${(request, request, request)}".to_string();
        let output_expr = from_string(expr_str.clone()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_number_float() {
        let input_expr = Expr::Number(InnerNumber::Float(1.1));
        let expr_str = to_string(&input_expr).unwrap();
        let output_expr = from_string(expr_str).unwrap();
        assert_eq!(input_expr, output_expr);
    }

    #[test]
    fn test_round_trip_read_write_number_u64() {
        let input_expr = Expr::Number(InnerNumber::UnsignedInteger(1));
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${1}".to_string();
        let output_expr = from_string(expr_str.clone()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_number_i64() {
        let input_expr = Expr::Number(InnerNumber::Integer(-1));
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${-1}".to_string();
        let output_expr = from_string(expr_str.clone()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_flags() {
        let input_expr = Expr::Flags(vec![
            "flag1".to_string(),
            "flag2".to_string(),
            "flag3".to_string(),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${{flag1, flag2, flag3}}".to_string();
        let output_expr = from_string(expr_str.clone()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_variable() {
        let input_expr = Expr::Variable("variable".to_string());
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${variable}".to_string();
        let output_expr = from_string(expr_str.clone()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_boolean() {
        let input_expr = Expr::Boolean(true);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str = "${true}".to_string();
        let output_expr = from_string(expr_str.clone()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    #[test]
    fn test_round_trip_read_write_record_of_sequence() {
        let input_expr = Expr::Record(vec![
            (
                "field".to_string(),
                Box::new(Expr::Sequence(vec![Expr::Request(), Expr::Request()])),
            ),
            (
                "field".to_string(),
                Box::new(Expr::Sequence(vec![Expr::Request(), Expr::Request()])),
            ),
            (
                "field".to_string(),
                Box::new(Expr::Sequence(vec![Expr::Request(), Expr::Request()])),
            ),
        ]);
        let expr_str = to_string(&input_expr).unwrap();
        let expected_str =
            "${{field: [request, request], field: [request, request], field: [request, request]}}"
                .to_string();
        let output_expr = from_string(expr_str.clone()).unwrap();
        assert_eq!((expr_str, input_expr), (expected_str, output_expr));
    }

    // Text based test
    #[test]
    fn test_sequence_of_records_singleton() {
        let expr_string = "${[{bc: request}]}";
        let output_expr = from_string(expr_string).unwrap();
        let expected_expr = Expr::Sequence(vec![Expr::Record(vec![(
            "bc".to_string(),
            Box::new(Expr::Request()),
        )])]);
        assert_eq!(output_expr, expected_expr);
    }

    #[test]
    fn test_sequence_of_records_multiple() {
        let expr_string = "${[{bc: request}, {cd: request}]}";
        let output_expr = from_string(expr_string).unwrap();
        let expected_expr = Expr::Sequence(vec![
            Expr::Record(vec![("bc".to_string(), Box::new(Expr::Request()))]),
            Expr::Record(vec![("cd".to_string(), Box::new(Expr::Request()))]),
        ]);
        assert_eq!(output_expr, expected_expr);
    }

    #[test]
    fn test_sequence_of_sequence_singleton() {
        let expr_string = "${[[bc]]}";
        let output_expr = from_string(expr_string).unwrap();
        let expected_expr = Expr::Sequence(vec![Expr::Sequence(vec![Expr::Variable("bc".to_string())])]);
        assert_eq!(output_expr, expected_expr);
    }

    #[test]
    fn test_sequence_of_sequence_multiple() {
        let expr_string = "${[[bc], [cd]]}";
        let output_expr = from_string(expr_string).unwrap();
        let expected_expr = Expr::Sequence(vec![
            Expr::Sequence(vec![Expr::Variable("bc".to_string())]),
            Expr::Sequence(vec![Expr::Variable("cd".to_string())]),
        ]);
        assert_eq!(output_expr, expected_expr);
    }

    #[test]
    fn test_sequence_of_tuple_singleton() {
        let expr_string = "${[(bc)]}";
        let output_expr = from_string(expr_string).unwrap();
        let expected_expr = Expr::Sequence(vec![Expr::Tuple(vec![Expr::Variable("bc".to_string())])]);
        assert_eq!(output_expr, expected_expr);
    }

    #[test]
    fn test_sequence_of_tuple_multiple() {
        let expr_string = "${[(bc), (cd)]}";
        let output_expr = from_string(expr_string).unwrap();
        let expected_expr = Expr::Sequence(vec![
            Expr::Tuple(vec![Expr::Variable("bc".to_string())]),
            Expr::Tuple(vec![Expr::Variable("cd".to_string())]),
        ]);
        assert_eq!(output_expr, expected_expr);
    }
}
