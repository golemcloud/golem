// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.

use crate::golem_agentic::golem::agent::common::{
    AuthDetails, CorsOptions, HttpMountDetails, PathSegment, PathVariable, SystemVariable,
};
use std::fmt;

#[derive(Debug)]
pub struct ParseError(pub String);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ParseError: {}", self.0)
    }
}

impl std::error::Error for ParseError {}

fn reject_query_param_in_string(path: &str, entity_name: &str) -> Result<(), String> {
    if path.contains('?') {
        return Err(format!("{} cannot contain query parameters", entity_name));
    }

    Ok(())
}

pub fn make_http_mount_details(
    path: &str,
    auth: bool,
    phantom_agent: bool,
    cors_options: CorsOptions,
    web_suffix: Option<String>,
) -> Result<HttpMountDetails, String> {
    reject_query_param_in_string(path, "HTTP mount path")?;

    let segments = parse_path(path).map_err(|e| e.to_string())?;

    let web_suffix = match web_suffix {
        Some(suffix) => {
            reject_query_param_in_string(&suffix, "webhook_suffix")?;

            let parsed_suffix = parse_path(&suffix).map_err(|e| e.to_string())?;

            if parsed_suffix.is_empty() {
                return Err("webhook_suffix cannot be empty if provided".to_string());
            }

            parsed_suffix
        }
        None => vec![],
    };

    Ok(HttpMountDetails {
        path_prefix: segments,
        auth_details: Some(AuthDetails { required: auth }),
        phantom_agent,
        cors_options: cors_options.clone(),
        webhook_suffix: web_suffix,
    })
}

fn reject_empty_string(name: &str, entity_name: &str) -> Result<(), ParseError> {
    if name.is_empty() {
        return Err(ParseError(format!("{} cannot be empty", entity_name)));
    }
    Ok(())
}

pub fn parse_path(path: &str) -> Result<Vec<PathSegment>, ParseError> {
    if !path.starts_with('/') {
        return Err(ParseError("HTTP mount must start with '/'".to_string()));
    }

    let segments: Vec<&str> = path.split('/').skip(1).collect();
    let mut parsed = Vec::with_capacity(segments.len());

    for (i, segment) in segments.iter().enumerate() {
        let is_last = i == segments.len() - 1;
        parsed.push(parse_segment(segment, is_last)?);
    }

    Ok(parsed)
}

fn parse_segment(segment: &str, is_last: bool) -> Result<PathSegment, ParseError> {
    if segment.is_empty() {
        return Err(ParseError(
            "Empty path segment (\"//\") is not allowed".to_string(),
        ));
    }

    if segment != segment.trim() {
        return Err(ParseError(
            "Whitespace is not allowed in path segments".to_string(),
        ));
    }

    if segment.starts_with('{') && segment.ends_with('}') {
        let name = &segment[1..segment.len() - 1];

        reject_empty_string(name, "path variable")?;

        if name.starts_with('*') {
            if !is_last {
                return Err(ParseError(format!(
                    "Remaining path variable \"{}\" is only allowed as the last path segment",
                    name
                )));
            }
            let variable_name = &name[1..];
            reject_empty_string(variable_name, "remaining path variable")?;
            return Ok(PathSegment::RemainingPathVariable(PathVariable {
                variable_name: variable_name.to_string(),
            }));
        }

        match name {
            "agent-type" => Ok(PathSegment::SystemVariable(SystemVariable::AgentType)),
            "agent-version" => Ok(PathSegment::SystemVariable(SystemVariable::AgentVersion)),
            _ => Ok(PathSegment::PathVariable(PathVariable {
                variable_name: name.to_string(),
            })),
        }
    } else if segment.contains('{') || segment.contains('}') {
        return Err(ParseError(format!(
            "Path segment \"{}\" must be a whole variable like \"{{id}}\" and cannot mix literals and variables",
            segment
        )));
    } else {
        reject_empty_string(segment, "Literal path segment")?;
        Ok(PathSegment::Literal(segment.to_string()))
    }
}

#[cfg(test)]
mod tests {

    use crate::agentic::parse_path;
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
