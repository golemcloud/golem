// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::log::LogColorize;
use anyhow::{anyhow, bail};
use chrono::{DateTime, Utc};
use golem_client::model::ScanCursor;
use golem_common::model::agent_secret::AgentSecretPath;
use golem_common::model::worker::AgentConfigEntryDto;

pub fn parse_key_val(key_and_val: &str) -> anyhow::Result<(String, String)> {
    let pos = key_and_val.find('=').ok_or_else(|| {
        anyhow!(
            "invalid KEY=VALUE: no `=` found in `{}`",
            key_and_val.log_color_error_highlight()
        )
    })?;
    Ok((
        key_and_val[..pos].to_string(),
        key_and_val[pos + 1..].to_string(),
    ))
}

pub fn parse_agent_config(s: &str) -> anyhow::Result<AgentConfigEntryDto> {
    let (path, value) = split_agent_config_path_and_value(s)?;

    let path = parse_agent_config_path(path)?;

    let value: serde_json::Value = serde_json::from_str(value)?;

    Ok(AgentConfigEntryDto {
        path,
        value: value.into(),
    })
}

pub fn parse_agent_secret_path(input: &str) -> anyhow::Result<AgentSecretPath> {
    Ok(AgentSecretPath(parse_agent_config_path(input)?))
}

fn split_agent_config_path_and_value(input: &str) -> anyhow::Result<(&str, &str)> {
    let chars = input.char_indices();
    let mut in_quotes = false;
    let mut escape = false;

    for (i, c) in chars {
        if escape {
            escape = false;
            continue;
        }

        match c {
            '\\' => escape = true,
            '"' => in_quotes = !in_quotes,
            '=' if !in_quotes => {
                let key = &input[..i];
                let value = &input[i + 1..];
                return Ok((key, value));
            }
            _ => {}
        }
    }

    Err(anyhow!("expected unescaped '=' separating key and value"))
}

fn parse_agent_config_path(input: &str) -> anyhow::Result<Vec<String>> {
    let mut keys = Vec::new();
    let mut buf = String::new();

    let mut chars = input.chars().peekable();
    let mut in_quotes = false;

    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                // escape next char
                let next = chars.next().ok_or_else(|| anyhow!("dangling escape"))?;
                buf.push(next);
            }

            '"' => {
                in_quotes = !in_quotes;
            }

            '.' if !in_quotes => {
                push_agent_config_path_segment(&mut keys, &mut buf)?;
            }

            _ => buf.push(c),
        }
    }

    if in_quotes {
        return Err(anyhow!("unterminated quote"));
    }

    push_agent_config_path_segment(&mut keys, &mut buf)?;

    Ok(keys)
}

fn push_agent_config_path_segment(keys: &mut Vec<String>, buf: &mut String) -> anyhow::Result<()> {
    let segment = buf.trim();

    if segment.is_empty() {
        return Err(anyhow!("empty path segment"));
    }

    keys.push(segment.to_string());
    buf.clear();

    Ok(())
}

// TODO: better error context and messages
pub fn parse_cursor(cursor: &str) -> anyhow::Result<ScanCursor> {
    let parts = cursor.split('/').collect::<Vec<_>>();

    if parts.len() != 2 {
        bail!("Invalid cursor format: {}", cursor);
    }

    Ok(ScanCursor {
        layer: parts[0].parse()?,
        cursor: parts[1].parse()?,
    })
}

pub fn parse_instant(
    s: &str,
) -> Result<DateTime<Utc>, Box<dyn std::error::Error + Send + Sync + 'static>> {
    match s.parse::<DateTime<Utc>>() {
        Ok(dt) => Ok(dt),
        Err(err) => Err(err.into()),
    }
}

#[cfg(test)]
mod parse_agent_config_tests {
    use super::{parse_agent_config, parse_agent_config_path};
    use golem_common::model::worker::AgentConfigEntryDto;
    use serde_json::json;
    use test_r::test;

    fn parse(input: &str) -> AgentConfigEntryDto {
        parse_agent_config(input).unwrap()
    }

    #[test]
    fn simple_path() {
        let e = parse(r#"a.b.c=1"#);

        assert_eq!(e.path, vec!["a", "b", "c"]);
        assert_eq!(e.value, json!(1).into());
    }

    #[test]
    fn string_value() {
        let e = parse(r#"a.b="hello""#);

        assert_eq!(e.path, vec!["a", "b"]);
        assert_eq!(e.value, json!("hello").into());
    }

    #[test]
    fn json_object_value() {
        let e = parse(r#"a.b={"x":1,"y":2}"#);

        assert_eq!(e.path, vec!["a", "b"]);
        assert_eq!(e.value, json!({"x":1,"y":2}).into());
    }

    #[test]
    fn quoted_path_segment() {
        let e = parse(r#""foo.bar".baz=1"#);

        assert_eq!(e.path, vec!["foo.bar", "baz"]);
        assert_eq!(e.value, json!(1).into());
    }

    #[test]
    fn quoted_segment_with_spaces() {
        let e = parse(r#""foo bar".baz=1"#);

        assert_eq!(e.path, vec!["foo bar", "baz"]);
        assert_eq!(e.value, json!(1).into());
    }

    #[test]
    fn escaped_dot_in_segment() {
        let e = parse(r#"foo\.bar.baz=1"#);

        assert_eq!(e.path, vec!["foo.bar", "baz"]);
        assert_eq!(e.value, json!(1).into());
    }

    #[test]
    fn equals_inside_value() {
        let e = parse(r#"a.b="foo=bar""#);

        assert_eq!(e.path, vec!["a", "b"]);
        assert_eq!(e.value, json!("foo=bar").into());
    }

    #[test]
    fn equals_inside_path() {
        let e = parse(r#""foo=bar".baz=1"#);

        assert_eq!(e.path, vec!["foo=bar", "baz"]);
        assert_eq!(e.value, json!(1).into());
    }

    #[test]
    fn escaped_equals_in_path() {
        let e = parse(r#"foo\=bar.baz=1"#);

        assert_eq!(e.path, vec!["foo=bar", "baz"]);
        assert_eq!(e.value, json!(1).into());
    }

    #[test]
    fn complex_case() {
        let e = parse(r#""foo.bar=baz"."x.y"={"hello":"world"}"#);

        assert_eq!(e.path, vec!["foo.bar=baz", "x.y"]);
        assert_eq!(e.value, json!({"hello":"world"}).into());
    }

    #[test]
    fn split_fails_without_equals() {
        let err = parse_agent_config("a.b.c").unwrap_err();
        assert!(err.to_string().contains("expected unescaped '='"));
    }

    #[test]
    fn unterminated_quote_in_path() {
        let err = parse_agent_config_path(r#""foo.bar.baz"#).unwrap_err();
        assert!(err.to_string().contains("unterminated quote"));
    }

    #[test]
    fn dangling_escape() {
        let err = parse_agent_config_path(r#"foo.bar\"#).unwrap_err();
        assert!(err.to_string().contains("dangling escape"));
    }
}
