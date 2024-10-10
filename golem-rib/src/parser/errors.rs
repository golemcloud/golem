use std::fmt::Display;

use serde::de::StdError;

// Custom error type to hold specific error messages within individual parser
// which later gets converted to StreamError
#[derive(Debug, PartialEq, Clone)]
pub enum RibParseError {
    Message(String),
}

impl Display for RibParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RibParseError::Message(msg) => write!(f, "{}", msg),
        }
    }
}

impl StdError for RibParseError {
    fn description(&self) -> &str {
        match self {
            RibParseError::Message(msg) => msg,
        }
    }
}

// A curated list of most common syntax errors, with the intent
// to not regress user-facing error messages with changing parsing logic
#[cfg(test)]
mod invalid_syntax_tests {
    use test_r::test;

    use crate::Expr;

    #[test]
    fn dangling_some_in_rib_program() {
        let input = r#"
          let x = 1;
          let y = 2;
          let z = 3;
          let result = some [x);
          result"#;
        let result = Expr::from_text(input);
        let expected_error = [
            "Parse error at line: 5, column: 24",
            "some is a keyword",
            "Invalid identifier",
            "",
        ]
        .join("\n");

        assert_eq!(result, Err(expected_error));
    }

    #[test]
    fn dangling_ok_in_rib_program() {
        let input = r#"
          let x = 1;
          let y = 2;
          let z = 3;
          let result = ok [x);
          result"#;
        let result = Expr::from_text(input);
        let expected_error = [
            "Parse error at line: 5, column: 24",
            "ok is a keyword",
            "Invalid identifier",
            "",
        ]
        .join("\n");

        assert_eq!(result, Err(expected_error));
    }

    #[test]
    fn dangling_err_in_rib_program() {
        let input = r#"
          let x = 1;
          let y = 2;
          let z = 3;
          let result = err [x);
          result"#;
        let result = Expr::from_text(input);
        let expected_error = [
            "Parse error at line: 5, column: 24",
            "err is a keyword",
            "Invalid identifier",
            "",
        ]
        .join("\n");

        assert_eq!(result, Err(expected_error));
    }

    #[test]
    fn invalid_conditional_in_rib_program() {
        let input = r#"
          let x = 1;
          let y = 2;
          let z = 3;
          let result = { if x > y 1 else 0 };
          result"#;
        let result = Expr::from_text(input);
        let expected_error = [
            "Parse error at line: 5, column: 35",
            "Unexpected `1`",
            "Expected whitespace or then",
            "",
        ]
        .join("\n");

        assert_eq!(result, Err(expected_error));
    }

    #[test]
    fn invalid_flag_in_rib_program() {
        let input = r#"
          let x = 1;
          let y = 2;
          let z = 3;
          let result = {x, y, z;
          result"#;
        let result = Expr::from_text(input);
        let expected_error = [
            "Parse error at line: 5, column: 26",
            "Unexpected `,`",
            "Expected whitespace or `}`",
            "",
        ]
        .join("\n");

        assert_eq!(result, Err(expected_error));
    }

    #[test]
    fn invalid_pattern_match_in_rib_program() {
        let input = r#"
          let x = 1;
          let y = 2;
          let z = 3;
          let result = match x {
            ok(x) => x
            err(x) => x,
          };
          result"#;
        let result = Expr::from_text(input);
        let expected_error = [
            "Parse error at line: 7, column: 13",
            "Unexpected `e`",
            "Expected whitespace or `}`",
            "Invalid syntax for pattern match",
            "",
        ]
        .join("\n");

        assert_eq!(result, Err(expected_error));
    }

    #[test]
    fn invalid_record_in_rib_program() {
        let input = r#"
          let x = 1;
          let y = 2;
          let z = 3;
          let result = {a : b, c : d;
          result"#;
        let result = Expr::from_text(input);
        let expected_error = [
            "Parse error at line: 5, column: 27",
            "Unexpected `:`",
            "Expected whitespace or `}`",
            "",
        ]
        .join("\n");

        assert_eq!(result, Err(expected_error));
    }

    #[test]
    fn invalid_sequence_in_rib_program() {
        let input = r#"
          let x = 1;
          let y = 2;
          let z = 3;
          let result = [x, y, z;
          result"#;
        let result = Expr::from_text(input);
        let expected_error = [
            "Parse error at line: 5, column: 32",
            "Unexpected `;`",
            "Expected `,`, whitespaces or `]`",
            "Invalid syntax for sequence type",
            "",
        ]
        .join("\n");

        assert_eq!(result, Err(expected_error));
    }

    #[test]
    fn invalid_tuple_in_rib_program() {
        let input = r#"
          let x = 1;
          let y = 2;
          let z = 3;
          let result = (x, y, z;
          result"#;
        let result = Expr::from_text(input);
        let expected_error = [
            "Parse error at line: 5, column: 32",
            "Unexpected `;`",
            "Expected `,`, whitespaces or `)`",
            "Invalid syntax for tuple type",
            "",
        ]
        .join("\n");

        assert_eq!(result, Err(expected_error));
    }

    #[test]
    fn missing_arrow_in_pattern_match() {
        let input = r#"match foo {
            ok(x) x,
            err(x) => x,
            _ => bar,
          }"#;
        let result = Expr::from_text(input);
        let expected_error = [
            "Parse error at line: 2, column: 19",
            "Unexpected `x`",
            "Expected whitespace or =>",
            "Invalid syntax for pattern match",
            "",
        ]
        .join("\n");

        assert_eq!(result, Err(expected_error));
    }

    #[test]
    fn missing_closing_bracket_in_err() {
        let input = r#"err(1"#;
        let result = Expr::from_text(input);
        let expected_error = [
            "Parse error at line: 1, column: 6",
            "Unexpected end of input",
            "Expected whitespace or `)`",
            "Invalid syntax for Result type",
            "",
        ]
        .join("\n");

        assert_eq!(result, Err(expected_error));
    }

    #[test]
    fn missing_closing_bracket_in_ok() {
        let input = r#"ok(1"#;
        let result = Expr::from_text(input);
        let expected_error = [
            "Parse error at line: 1, column: 5",
            "Unexpected end of input",
            "Expected whitespace or `)`",
            "Invalid syntax for Result type",
            "",
        ]
        .join("\n");

        assert_eq!(result, Err(expected_error));
    }

    #[test]
    fn missing_closing_bracket_in_some() {
        let input = r#"some(1"#;
        let result = Expr::from_text(input);
        let expected_error = [
            "Parse error at line: 1, column: 7",
            "Unexpected end of input",
            "Expected whitespace or `)`",
            "Invalid syntax for Option type",
            "",
        ]
        .join("\n");

        assert_eq!(result, Err(expected_error));
    }

    #[test]
    fn missing_closing_braces_in_pattern_match() {
        let input = r#"match foo {
            ok(x) => x,
            err(x) => x,
            _ => bar
          "#;
        let result = Expr::from_text(input);
        let expected_error = [
            "Parse error at line: 5, column: 11",
            "Unexpected end of input",
            "Expected whitespace or `}`",
            "Invalid syntax for pattern match",
            "",
        ]
        .join("\n");

        assert_eq!(result, Err(expected_error));
    }

    #[test]
    fn missing_comma_in_pattern_match() {
        let input = r#"match foo {
            ok(x) => x
            err(x) => x,
            _ => bar,
          }"#;
        let result = Expr::from_text(input);
        let expected_error = [
            "Parse error at line: 3, column: 13",
            "Unexpected `e`",
            "Expected whitespace or `}`",
            "Invalid syntax for pattern match",
            "",
        ]
        .join("\n");

        assert_eq!(result, Err(expected_error));
    }

    #[test]
    fn missing_opening_braces_in_pattern_match() {
        let input = r#"match foo
            ok(x) => x,
            err(x) => x,
            _ => bar,
          }"#;
        let result = Expr::from_text(input);
        let expected_error = [
            "Parse error at line: 2, column: 13",
            "Unexpected `o`",
            "Expected whitespace or `{`",
            "Invalid syntax for pattern match",
            "",
        ]
        .join("\n");

        assert_eq!(result, Err(expected_error));
    }

    #[test]
    fn missing_semi_column() {
        let input = r#"
          let x = 1;
          let y = 2
          y"#;
        let result = Expr::from_text(input);
        let expected_error = [
            "Parse error at line: 4, column: 11",
            "Unexpected `y`",
            "Expected `;`, whitespaces or end of input",
            "",
        ]
        .join("\n");

        assert_eq!(result, Err(expected_error));
    }

    #[test]
    fn missing_then_in_conditional() {
        let input = r#"if x 1 else 2"#;
        let result = Expr::from_text(input);
        let expected_error = [
            "Parse error at line: 1, column: 6",
            "Unexpected `1`",
            "Expected whitespace or then",
            "",
        ]
        .join("\n");

        assert_eq!(result, Err(expected_error));
    }

    #[test]
    fn redundant_parenthesis_in_none() {
        let input = r#"none()"#;
        let result = Expr::from_text(input);
        let expected_error = [
            "Parse error at line: 1, column: 5",
            "Unexpected `(`",
            "Expected `;`, whitespaces or end of input",
            "",
        ]
        .join("\n");

        assert_eq!(result, Err(expected_error));
    }
}
