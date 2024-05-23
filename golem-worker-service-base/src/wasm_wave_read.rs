use crate::evaluator::{DefaultEvaluator, EvaluationContext, Evaluator};
use crate::expression;
use golem_wasm_rpc::{TypeAnnotatedValue, Value};

// A handy utility that allows you to read string of wasm_syntax to wasm value (golem_wasm_rpc::value::Value)
// without explicitly passing `AnalysedType` unlike wasm_wave functionalities. It may not be used in the wild.
pub async fn read_wasm_wave_string(wave_syntax_str: impl AsRef<str>) -> Result<Value, String> {
    let typed_value = read_wasm_wave_string_internal(wave_syntax_str)?;
    Value::try_from(typed_value).map_err(|e| e.to_string())
}

async fn read_wasm_wave_string_internal(
    wave_syntax_str: impl AsRef<str>,
) -> Result<TypeAnnotatedValue, String> {
    let expr = expression::from_string(wave_syntax_str).map_err(|e| e.to_string())?;
    let noop_executor = DefaultEvaluator::noop();
    let result = noop_executor
        .evaluate(&expr, &EvaluationContext::empty())
        .await
        .map_err(|err| err.to_string())?;

    result
        .get_value()
        .ok_or("Unable to evaluate wave_syntax".to_string())
}

#[cfg(test)]
mod tests {
    use crate::wasm_wave_read::read_wasm_wave_string;
    use golem_wasm_rpc::Value;

    fn wrap_in_code_interpolation(wasm_wave_string: &str) -> String {
        format!("${{{}}}", wasm_wave_string)
    }

    #[tokio::test]
    async fn test_read_wasm_wave_number() {
        let wasm_wave_string = "1";
        let result = read_wasm_wave_string(wrap_in_code_interpolation(wasm_wave_string)).unwrap();
        assert_eq!(result, golem_wasm_rpc::Value::U64(1));
    }

    #[tokio::test]
    async fn test_read_wasm_wave_record_of_constructors() {
        let wasm_wave_string = "{ a : ok(1), b : err(2) }";
        let result = read_wasm_wave_string(wrap_in_code_interpolation(wasm_wave_string)).unwrap();
        let expected = Value::Record(vec![
            Value::Result(Ok(Some(Box::new(Value::U64(1))))),
            Value::Result(Err(Some(Box::new(Value::U64(2))))),
        ]);

        assert_eq!(result, expected);
    }

    #[tokio::test]
    async fn test_read_wasm_wave_of_sequence() {
        let wasm_wave_string = "[1, 2, 3]";
        let result = read_wasm_wave_string(wrap_in_code_interpolation(wasm_wave_string)).unwrap();
        let expected = Value::List(vec![Value::U64(1), Value::U64(2), Value::U64(3)]);
        assert_eq!(result, expected);
    }

    #[tokio::test]
    async fn test_read_wasm_wave_of_tuple() {
        let wasm_wave_string = "(1, 2, 3)";
        let result = read_wasm_wave_string(wrap_in_code_interpolation(wasm_wave_string)).unwrap();
        let expected = Value::Tuple(vec![Value::U64(1), Value::U64(2), Value::U64(3)]);
        assert_eq!(result, expected);
    }
}
