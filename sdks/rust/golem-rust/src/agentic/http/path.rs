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

use crate::agentic::http::validations::reject_empty_string;
use crate::golem_agentic::golem::agent::common::{PathSegment, PathVariable, SystemVariable};

pub fn parse_path(path: &str) -> Result<Vec<PathSegment>, String> {
    if !path.starts_with('/') {
        return Err("HTTP mount must start with '/'".to_string());
    }

    if path == "/" {
        return Ok(Vec::new());
    }

    let segments: Vec<&str> = path.split('/').skip(1).collect();
    let mut parsed = Vec::with_capacity(segments.len());

    for (i, segment) in segments.iter().enumerate() {
        let is_last = i == segments.len() - 1;
        parsed.push(parse_segment(segment, is_last)?);
    }

    Ok(parsed)
}

pub fn parse_segment(segment: &str, is_last: bool) -> Result<PathSegment, String> {
    if segment.is_empty() {
        return Err("Empty path segment (\"//\") is not allowed".to_string());
    }

    if segment != segment.trim() {
        return Err("Whitespace is not allowed in path segments".to_string());
    }

    if segment.starts_with('{') && segment.ends_with('}') {
        let name = &segment[1..segment.len() - 1];

        reject_empty_string(name, "path variable")?;

        if name.starts_with('*') {
            if !is_last {
                return Err(format!(
                    "Remaining path variable \"{}\" is only allowed as the last path segment",
                    name
                ));
            }
            if let Some(variable_name) = name.strip_prefix('*') {
                reject_empty_string(variable_name, "remaining path variable")?;
                return Ok(PathSegment::RemainingPathVariable(PathVariable {
                    variable_name: variable_name.to_string(),
                }));
            }
        }

        match name {
            "agent-type" => Ok(PathSegment::SystemVariable(SystemVariable::AgentType)),
            "agent-version" => Ok(PathSegment::SystemVariable(SystemVariable::AgentVersion)),
            _ => Ok(PathSegment::PathVariable(PathVariable {
                variable_name: name.to_string(),
            })),
        }
    } else if segment.contains('{') || segment.contains('}') {
        Err(format!(
            "Path segment \"{}\" must be a whole variable like \"{{id}}\" and cannot mix literals and variables",
            segment
        ))
    } else {
        reject_empty_string(segment, "Literal path segment")?;
        Ok(PathSegment::Literal(segment.to_string()))
    }
}

#[cfg(test)]
mod tests {

    use crate::agentic::http::path::parse_path;
    use crate::golem_agentic::golem::agent::common::{PathSegment, PathVariable};
    use test_r::test;

    #[test]
    fn test_parse_path_basic() {
        let path = "/foo/bar/{id}";
        let parsed = parse_path(path).unwrap();
        assert_eq!(parsed.len(), 3);
        match &parsed[2] {
            PathSegment::PathVariable(PathVariable { variable_name }) => {
                assert_eq!(variable_name, "id")
            }
            _ => panic!("expected PathVariable"),
        }
    }

    #[test]
    fn test_parse_path_remaining_variable() {
        let path = "/foo/{*rest}";
        let parsed = parse_path(path).unwrap();
        match &parsed[1] {
            PathSegment::RemainingPathVariable(PathVariable { variable_name }) => {
                assert_eq!(variable_name, "rest")
            }
            _ => panic!("expected RemainingPathVariable"),
        }
    }
}
