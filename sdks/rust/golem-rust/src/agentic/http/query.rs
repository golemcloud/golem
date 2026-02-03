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

            if value != value.trim() {
                return Err("Whitespace is not allowed in query variables".to_string());
            }

            if !value.starts_with('{') || !value.ends_with('}') {
                return Err(format!(
                    r#"Query value for "{}" must be a variable reference"#,
                    key
                ));
            }

            let variable_name = &value[1..value.len() - 1];

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
