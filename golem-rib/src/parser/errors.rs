// A curated list of most common syntax errors, with the intent
// not regress user-facing error messages with changing parsing logic
#[cfg(test)]
mod error_tests {

    use crate::Expr;

    #[test]
    fn test_pattern_match_error_missing_opening_curly_brace() {
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
    fn test_pattern_match_error_missing_closing_curly_brace() {
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

    // TODO; Missing comma since we have multiple arms is a better error message
    // This requires change in parsing logic to avoid using sep_by1
    #[test]
    fn test_pattern_match_error_missing_comma_between_arms() {
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
    fn test_pattern_match_error_missing_arrow() {
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
    fn test_let_binding_error_missing_variable() {
        let input = r#"let = 1;"#;
        let result = Expr::from_text(input);
        let expected_error = [
            "Parse error at line: 1, column: 5",
            "Unexpected `=`",
            "Expected whitespace, letter, digit or `_`",
            "Unable to parse binding variable",
            "",
        ]
        .join("\n");

        assert_eq!(result, Err(expected_error));
    }

    #[test]
    fn test_let_binding_error_missing_assignment() {
        let input = r#"let x 1;"#;
        let result = Expr::from_text(input);
        let expected_error = [
            "Parse error at line: 1, column: 7",
            "Unexpected `1`",
            "Expected whitespace, `:`, whitespaces, bool, s8, u8, s16, u16, s32, u32, s64, u64, f32, f64, chr, str, list, tuple, option or `=`",
            ""
        ].join("\n");

        assert_eq!(result, Err(expected_error));
    }
    #[test]
    fn test_conditional_no_then() {
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
    fn test_result_ok_missing_braces() {
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
    fn test_result_err_missing_braces() {
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
    fn test_option_some_missing_braces() {
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
    fn test_option_none_redundant_braces() {
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

    #[test]
    fn test_syntax_error_in_rib_program_missing_semi_column() {
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
    fn test_syntax_error_in_rib_program_let_statement() {
        let input = r#"
          let x = 1;
          let y = 2;
          let z = 3;
          let"#;
        let result = Expr::from_text(input);
        let expected_error = [
            "Parse error at line: 5, column: 14",
            "Unexpected end of input",
            "Expected whitespace, letter, digit or `_`",
            "Unable to parse binding variable",
            "",
        ]
        .join("\n");

        assert_eq!(result, Err(expected_error));
    }

    #[test]
    fn test_syntax_error_in_rib_program_if_cond() {
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
    fn test_syntax_error_in_rib_program_pattern_match() {
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
    fn test_syntax_error_in_rib_program_err() {
        let input = r#"
          let x = 1;
          let y = 2;
          let z = 3;
          let result = err1(x);
          result"#;
        let result = Expr::from_text(input);
        let expected_error = [
            "Parse error at line: 5, column: 27",
            "Unexpected `1`",
            "Expected `(`",
            "Invalid syntax for Result type",
            "",
        ]
        .join("\n");

        assert_eq!(result, Err(expected_error));
    }

    #[test]
    fn test_syntax_error_in_rib_program_ok() {
        let input = r#"
          let x = 1;
          let y = 2;
          let z = 3;
          let result = ok1(x);
          result"#;
        let result = Expr::from_text(input);
        let expected_error = [
            "Parse error at line: 5, column: 26",
            "Unexpected `1`",
            "Expected `(`",
            "Invalid syntax for Result type",
            "",
        ]
        .join("\n");

        assert_eq!(result, Err(expected_error));
    }

    #[test]
    fn test_syntax_error_in_rib_program_option_some() {
        let input = r#"
          let x = 1;
          let y = 2;
          let z = 3;
          let result = some1(x);
          result"#;
        let result = Expr::from_text(input);
        let expected_error = [
            "Parse error at line: 5, column: 28",
            "Unexpected `1`",
            "Expected `(`",
            "Invalid syntax for Option type",
            "",
        ]
        .join("\n");

        assert_eq!(result, Err(expected_error));
    }

    #[test]
    fn test_syntax_error_in_rib_program_invalid_tuple() {
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
    fn test_syntax_error_in_rib_program_invalid_sequence() {
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
    fn test_syntax_error_in_rib_program_invalid_flag() {
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
    fn test_syntax_error_in_rib_program_invalid_record() {
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
}
