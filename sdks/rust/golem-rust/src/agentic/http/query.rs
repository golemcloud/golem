// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::agentic::reject_empty_string;
use crate::golem_agentic::golem::agent::common::QueryVariable;

pub fn parse_query(query: &str) -> Result<Vec<QueryVariable>, String> {
    if query.is_empty() {
        return Ok(vec![]);
    }

    query
        .split('&')
        .map(|pair| {
            let mut parts = pair.splitn(2, '=');
            let key = parts.next().unwrap_or("");
            let value = parts.next().unwrap_or("");

            if key.is_empty() || value.is_empty() {
                return Err(format!(r#"Invalid query segment "{}""#, pair));
            }

            if !value.starts_with('{') || !value.ends_with('}') {
                return Err(format!(
                    r#"Query value for "{}" must be a variable reference"#,
                    key
                ));
            }

            let variable_name = &value[1..value.len() - 1];

            if variable_name != variable_name.trim() {
                return Err("Whitespace is not allowed in query variables".to_string());
            }

            reject_empty_string(
                variable_name,
                "Query variable name cannot be an empty string",
            )?;

            Ok(QueryVariable {
                query_param_name: key.to_string(),
                variable_name: variable_name.to_string(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    #[test]
    fn test_empty_query() {
        let res = parse_query("").unwrap();
        assert!(res.is_empty());
    }

    #[test]
    fn test_single_valid_query_variable() {
        let res = parse_query("x={var}").unwrap();
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].query_param_name, "x");
        assert_eq!(res[0].variable_name, "var");
    }

    #[test]
    fn test_multiple_valid_query_variables() {
        let res = parse_query("x={var}&y={another}").unwrap();
        assert_eq!(res.len(), 2);
        assert_eq!(res[0].query_param_name, "x");
        assert_eq!(res[0].variable_name, "var");
        assert_eq!(res[1].query_param_name, "y");
        assert_eq!(res[1].variable_name, "another");
    }

    #[test]
    fn test_invalid_missing_value() {
        let err = parse_query("x=").unwrap_err();
        assert!(err.contains("Invalid query segment"));
    }

    #[test]
    fn test_invalid_missing_key() {
        let err = parse_query("={var}").unwrap_err();
        assert!(err.contains("Invalid query segment"));
    }

    #[test]
    fn test_whitespace_in_value() {
        let err = parse_query("x={ var }").unwrap_err();
        assert_eq!(err, "Whitespace is not allowed in query variables");
    }

    #[test]
    fn test_value_not_wrapped_in_braces() {
        let err = parse_query("x=var").unwrap_err();
        assert!(err.contains("must be a variable reference"));
    }

    #[test]
    fn test_empty_variable_name() {
        let err = parse_query("x={}").unwrap_err();
        assert!(err.contains("cannot be an empty string"));
    }

    #[test]
    fn test_complex_mixed_query() {
        let res = parse_query("foo={f}&bar={b}").unwrap();
        assert_eq!(res.len(), 2);
        assert_eq!(res[0].query_param_name, "foo");
        assert_eq!(res[0].variable_name, "f");
        assert_eq!(res[1].query_param_name, "bar");
        assert_eq!(res[1].variable_name, "b");
    }
}
