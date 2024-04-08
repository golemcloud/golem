use crate::evaluator::Evaluator;
use crate::expression;
use golem_wasm_rpc::{TypeAnnotatedValue, Value};

// A handy utility that allows you to read string of wasm_syntax to wasm value (golem_wasm_rpc::value::Value)
// without explicitly passing `AnalysedType` unlike wasm_wave functionalities.
// Note that,it is 90% compatible with wasm_wave syntax.
// The progression of this utility is to be fully compatible with wasm_wave syntax.
// The minor differences are every wasm syntax needs to be wrapped in a `${}` to work with this utility.
// Another difference, expr_parser currently uses `single quotes` for literals but wasm-wave uses `double-quotes`
// Not to mention wasm-wave syntax doesn't allow undefined variables such `request.path`, but those are features exclusive to `Expr`.
// In short, `Expr` is a superset of wasm_wave syntax, and we use same expression parser to read wasm_syntax string, and convert to `TypeAnnotatedValue`
pub fn read_wasm_wave_string(wasm_syntax_str: impl AsRef<str>) -> Result<Value, String> {
    let typed_value = read_wasm_wave_string_typed(wasm_syntax_str)?;
    Value::try_from(typed_value).map_err(|e| e.to_string())
}

// This will give type annotated
pub fn read_wasm_wave_string_typed(
    wasm_syntax_str: impl AsRef<str>,
) -> Result<TypeAnnotatedValue, String> {
    let expr = expression::from_string(wasm_syntax_str).map_err(|e| e.to_string())?;
    expr.evaluate(&TypeAnnotatedValue::Record {
        typ: vec![],
        value: vec![],
    })
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use crate::wasm_wave_read::read_wasm_wave_string;
    use golem_wasm_rpc::Value;

    fn wrap_in_code_interpolation(wasm_wave_string: &str) -> String {
        format!("${{{}}}", wasm_wave_string)
    }

    #[test]
    fn test_read_wasm_wave_number() {
        let wasm_wave_string = "1";
        let result = read_wasm_wave_string(wrap_in_code_interpolation(wasm_wave_string)).unwrap();
        assert_eq!(result, golem_wasm_rpc::Value::U64(1));
    }

    #[test]
    fn test_read_wasm_wave_record_of_constructors() {
        let wasm_wave_string = "{ a : ok(1), b : err(2) }";
        let result = read_wasm_wave_string(wrap_in_code_interpolation(wasm_wave_string)).unwrap();
        let expected = Value::Record(vec![
            Value::Result(Ok(Some(Box::new(Value::U64(1))))),
            Value::Result(Err(Some(Box::new(Value::U64(2))))),
        ]);

        assert_eq!(result, expected);
    }

    #[test]
    fn test_read_wasm_wave_of_sequence() {
        let wasm_wave_string = "[1, 2, 3]";
        let result = read_wasm_wave_string(wrap_in_code_interpolation(wasm_wave_string)).unwrap();
        let expected = Value::List(vec![Value::U64(1), Value::U64(2), Value::U64(3)]);
        assert_eq!(result, expected);
    }

    #[test]
    fn test_read_wasm_wave_of_tuple() {
        let wasm_wave_string = "(1, 2, 3)";
        let result = read_wasm_wave_string(wrap_in_code_interpolation(wasm_wave_string)).unwrap();
        let expected = Value::Tuple(vec![Value::U64(1), Value::U64(2), Value::U64(3)]);
        assert_eq!(result, expected);
    }
}
