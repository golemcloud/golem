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

//! Writers of a raw JSON text for each structured format.
//!
//! They read the text token by token and never give a number to a number type. So each number is
//! written as it is in the text, the members of an object keep their order, and no parser sets a
//! limit on the nesting.

use crate::toon::{ToonEncodeError, ToonValue};
use anyhow::{anyhow, bail};

/// One token of a JSON text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token<'a> {
    /// `{` or `[`.
    Open(char),
    /// `}` or `]`.
    Close(char),
    Comma,
    Colon,
    /// A string with its quotes, as it is in the text.
    String(&'a str),
    /// A number, `true`, `false` or `null`, as it is in the text.
    Scalar(&'a str),
}

/// The tokens of a JSON text. The text comes from a `RawValue`, so it is valid JSON.
struct Tokens<'a> {
    text: &'a str,
    position: usize,
}

fn tokens(text: &str) -> Tokens<'_> {
    Tokens { text, position: 0 }
}

fn is_white_space(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r')
}

impl<'a> Iterator for Tokens<'a> {
    type Item = Token<'a>;

    fn next(&mut self) -> Option<Token<'a>> {
        let bytes = self.text.as_bytes();
        while bytes
            .get(self.position)
            .copied()
            .is_some_and(is_white_space)
        {
            self.position += 1;
        }
        let start = self.position;
        let first = *bytes.get(start)?;
        let token = match first {
            b'{' | b'[' => {
                self.position += 1;
                Token::Open(first as char)
            }
            b'}' | b']' => {
                self.position += 1;
                Token::Close(first as char)
            }
            b',' => {
                self.position += 1;
                Token::Comma
            }
            b':' => {
                self.position += 1;
                Token::Colon
            }
            b'"' => {
                let mut end = start + 1;
                while end < bytes.len() && bytes[end] != b'"' {
                    // The byte after a backslash belongs to the escape, so an escaped quote does
                    // not end the string.
                    end += if bytes[end] == b'\\' { 2 } else { 1 };
                }
                self.position = (end + 1).min(bytes.len());
                Token::String(&self.text[start..self.position])
            }
            _ => {
                let mut end = start;
                while end < bytes.len()
                    && !is_white_space(bytes[end])
                    && !matches!(bytes[end], b',' | b':' | b'{' | b'}' | b'[' | b']' | b'"')
                {
                    end += 1;
                }
                self.position = end;
                Token::Scalar(&self.text[start..end])
            }
        };
        Some(token)
    }
}

/// The JSON text without the white space between its tokens, for one line of output.
pub fn compact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for token in tokens(text) {
        match token {
            Token::Open(bracket) | Token::Close(bracket) => out.push(bracket),
            Token::Comma => out.push(','),
            Token::Colon => out.push(':'),
            Token::String(token) | Token::Scalar(token) => out.push_str(token),
        }
    }
    out
}

/// The JSON text in the layout of `serde_json::to_string_pretty`, for a value that starts on a
/// line with `level` levels of indentation.
pub fn pretty(text: &str, level: usize) -> String {
    fn new_line(out: &mut String, level: usize) {
        out.push('\n');
        for _ in 0..level {
            out.push_str("  ");
        }
    }

    let mut out = String::with_capacity(text.len());
    let mut level = level;
    let mut tokens = tokens(text).peekable();
    while let Some(token) = tokens.next() {
        match token {
            Token::Open(open) => {
                out.push(open);
                if let Some(Token::Close(close)) = tokens.peek().copied() {
                    tokens.next();
                    out.push(close);
                } else {
                    level += 1;
                    new_line(&mut out, level);
                }
            }
            Token::Close(close) => {
                level = level.saturating_sub(1);
                new_line(&mut out, level);
                out.push(close);
            }
            Token::Comma => {
                out.push(',');
                new_line(&mut out, level);
            }
            Token::Colon => out.push_str(": "),
            Token::String(token) | Token::Scalar(token) => out.push_str(token),
        }
    }
    out
}

/// The string that a string token holds.
fn string_of(token: &str) -> anyhow::Result<String> {
    serde_json::from_str(token)
        .map_err(|error| anyhow!("a string of the raw JSON value is not valid Unicode: {error}"))
}

/// A YAML parser can refuse a longer member name that has no `?` before it.
const MAX_YAML_SIMPLE_KEY: usize = 1000;

/// The JSON text as one YAML flow node on one line.
///
/// A JSON text is not always valid YAML: YAML has no escape for a surrogate pair, and a parser
/// can limit the length of a member name. So each string is written again with the escapes of
/// YAML, and a long member name gets a `?`. A number, `true`, `false` and `null` are written as
/// they are in the text.
pub fn yaml_flow(text: &str) -> anyhow::Result<String> {
    let mut out = String::with_capacity(text.len());
    // One entry for each open container: true for an object.
    let mut containers: Vec<bool> = Vec::new();
    // True where the next string is a member name.
    let mut expects_name = false;
    let mut long_name = false;
    for token in tokens(text) {
        match token {
            Token::Open(open) => {
                out.push(open);
                containers.push(open == '{');
                expects_name = open == '{';
            }
            Token::Close(close) => {
                out.push(close);
                containers.pop();
                expects_name = false;
            }
            Token::Comma => {
                out.push_str(", ");
                expects_name = containers.last() == Some(&true);
            }
            Token::Colon => {
                out.push_str(if long_name { " : " } else { ": " });
                long_name = false;
                expects_name = false;
            }
            Token::String(token) => {
                let quoted = yaml_double_quoted(&string_of(token)?);
                if expects_name && quoted.len() > MAX_YAML_SIMPLE_KEY {
                    out.push_str("? ");
                    long_name = true;
                }
                out.push_str(&quoted);
            }
            Token::Scalar(scalar) => out.push_str(scalar),
        }
    }
    Ok(out)
}

/// A YAML double-quoted scalar on one line.
fn yaml_double_quoted(string: &str) -> String {
    let mut out = String::with_capacity(string.len() + 2);
    out.push('"');
    for character in string.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            character if yaml_writes_as_it_is(character) => out.push(character),
            character => {
                let code = character as u32;
                if code <= 0xff {
                    out.push_str(&format!("\\x{code:02x}"));
                } else if code <= 0xffff {
                    out.push_str(&format!("\\u{code:04x}"));
                } else {
                    out.push_str(&format!("\\U{code:08x}"));
                }
            }
        }
    }
    out.push('"');
    out
}

/// The characters that every YAML version takes in a scalar without an escape. The others are
/// control characters, the line breaks of YAML 1.1 (next line, line separator and paragraph
/// separator), the byte order mark and the two last code points of the first plane.
fn yaml_writes_as_it_is(character: char) -> bool {
    matches!(
        character as u32,
        0x20..=0x7e
            | 0xa0..=0x2027
            | 0x202a..=0xd7ff
            | 0xe000..=0xfefe
            | 0xff00..=0xfffd
            | 0x10000..=0x10ffff
    )
}

/// The deepest nesting that the TOON encoder accepts.
const MAX_TOON_DEPTH: usize = 1024;

/// The JSON text as a value for the TOON encoder: the members keep their order, and each number
/// is its text.
pub fn toon_value(text: &str) -> anyhow::Result<ToonValue> {
    enum Open {
        Array(Vec<ToonValue>),
        /// The members, and the name of the member whose value comes next.
        Object(Vec<(String, ToonValue)>, Option<String>),
    }

    let mut open: Vec<Open> = Vec::new();
    let mut result = None;
    for token in tokens(text) {
        let value = match token {
            Token::Open(bracket) => {
                if open.len() >= MAX_TOON_DEPTH {
                    return Err(ToonEncodeError::MaxDepthExceeded.into());
                }
                open.push(if bracket == '{' {
                    Open::Object(Vec::new(), None)
                } else {
                    Open::Array(Vec::new())
                });
                continue;
            }
            Token::Close(_) => match open.pop() {
                Some(Open::Array(items)) => ToonValue::Array(items),
                Some(Open::Object(members, _)) => ToonValue::Object(members),
                None => bail!("the raw JSON value closes a container that is not open"),
            },
            Token::Comma | Token::Colon => continue,
            Token::String(token) => {
                let string = string_of(token)?;
                if let Some(Open::Object(_, name @ None)) = open.last_mut() {
                    *name = Some(string);
                    continue;
                }
                ToonValue::String(string)
            }
            Token::Scalar("null") => ToonValue::Null,
            Token::Scalar("true") => ToonValue::Bool(true),
            Token::Scalar("false") => ToonValue::Bool(false),
            Token::Scalar(number) => ToonValue::Number(number.to_string()),
        };
        match open.last_mut() {
            Some(Open::Array(items)) => items.push(value),
            Some(Open::Object(members, name)) => {
                let name = name
                    .take()
                    .ok_or_else(|| anyhow!("a member of the raw JSON value has no name"))?;
                members.push((name, value));
            }
            None => result = Some(value),
        }
    }
    result.ok_or_else(|| anyhow!("the raw JSON text holds no value"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;

    /// A text with white space between the tokens, and with every structure character, an
    /// escaped quote and a backslash inside strings.
    const TEXT: &str = r#" { "a b" : [ 1 , 2.50 , 1e400 , -0 ] , "q\"{}[],:\\" : { } , "n" : null ,
        "t" : true , "e" : [ ] , "big" : 18446744073709551617 } "#;

    #[test]
    fn compact_removes_only_the_white_space_between_tokens() {
        assert_eq!(
            compact(TEXT),
            r#"{"a b":[1,2.50,1e400,-0],"q\"{}[],:\\":{},"n":null,"t":true,"e":[],"big":18446744073709551617}"#
        );
    }

    #[test]
    fn pretty_has_the_layout_of_serde_json() {
        // Every number of this text fits a `serde_json::Value`, so `serde_json` gives the layout
        // to compare with.
        let text = r#"{"a":[1,{"b":null,"c":[]},"x y"],"d":{},"e":{"f":true}}"#;
        let value: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(
            pretty(text, 0),
            serde_json::to_string_pretty(&value).unwrap()
        );

        assert_eq!(
            pretty(r#"{"n":1e400,"m":[2.50]}"#, 2),
            "{\n      \"n\": 1e400,\n      \"m\": [\n        2.50\n      ]\n    }"
        );
    }

    #[test]
    fn yaml_flow_is_yaml_with_the_numbers_of_the_text() {
        let flow = yaml_flow(TEXT).unwrap();
        assert_eq!(
            flow,
            r#"{"a b": [1, 2.50, 1e400, -0], "q\"{}[],:\\": {}, "n": null, "t": true, "e": [], "big": 18446744073709551617}"#
        );
        // A YAML parser reads it as a mapping with the same members in the same order. The
        // `Value` of `serde_yaml` cannot hold the last number, so it reads the text without it.
        let readable = flow.replace(", \"big\": 18446744073709551617", "");
        let read: serde_yaml::Value = serde_yaml::from_str(&readable).unwrap();
        let names: Vec<&str> = read
            .as_mapping()
            .unwrap()
            .keys()
            .map(|name| name.as_str().unwrap())
            .collect();
        assert_eq!(names, ["a b", "q\"{}[],:\\", "n", "t", "e"]);
    }

    #[test]
    fn yaml_flow_writes_strings_again_with_the_escapes_of_yaml() {
        // A surrogate pair, a line break of YAML 1.1 and a control character. YAML has no escape
        // for a surrogate pair, so the character itself is written.
        let name = "k".repeat(MAX_YAML_SIMPLE_KEY);
        let text = format!(
            "{{\"face\":\"{}\",\"break\":\"a{}b\",\"bell\":\"{}\",\"{name}\":1}}",
            "\\ud83d\\ude00", "\u{2028}", "\\u0007"
        );
        let flow = yaml_flow(&text).unwrap();
        assert!(flow.contains("\"face\": \"\u{1f600}\""), "{flow}");
        assert!(flow.contains("\"break\": \"a\\u2028b\""), "{flow}");
        assert!(flow.contains("\"bell\": \"\\x07\""), "{flow}");
        assert!(flow.contains(&format!("? \"{name}\" : 1")), "{flow}");
        let read: serde_yaml::Value = serde_yaml::from_str(&flow).unwrap();
        assert_eq!(read["face"], "\u{1f600}");
        assert_eq!(read["break"], "a\u{2028}b");
        assert_eq!(read["bell"], "\u{7}");
        assert_eq!(read[name.as_str()], 1);
    }

    #[test]
    fn a_string_that_is_not_valid_unicode_is_an_error_where_it_is_written_again() {
        let text = format!("{{\"lone\":\"{}\"}}", "\\ud800");
        assert!(yaml_flow(&text).is_err());
        assert!(toon_value(&text).is_err());
        // The JSON formats copy the string token, so they keep it.
        assert_eq!(compact(&text), text);
    }

    #[test]
    fn toon_value_keeps_the_order_and_the_number_texts() {
        assert_eq!(
            toon_value(r#"{"z":1e400,"a":[2.50,"x",null,true,{"b":-0}],"z":false}"#).unwrap(),
            ToonValue::Object(vec![
                ("z".to_string(), ToonValue::Number("1e400".to_string())),
                (
                    "a".to_string(),
                    ToonValue::Array(vec![
                        ToonValue::Number("2.50".to_string()),
                        ToonValue::String("x".to_string()),
                        ToonValue::Null,
                        ToonValue::Bool(true),
                        ToonValue::Object(vec![(
                            "b".to_string(),
                            ToonValue::Number("-0".to_string())
                        )]),
                    ])
                ),
                ("z".to_string(), ToonValue::Bool(false)),
            ])
        );
        assert_eq!(toon_value("7").unwrap(), ToonValue::Number("7".to_string()));
    }

    #[test]
    fn the_readers_have_no_limit_below_the_one_of_toon() {
        let deep = format!("{}1{}", "[".repeat(1000), "]".repeat(1000));
        assert_eq!(compact(&deep), deep);
        assert_eq!(yaml_flow(&deep).unwrap(), deep);
        assert!(toon_value(&deep).is_ok());
        assert_eq!(pretty(&deep, 0).lines().count(), 2001);

        let too_deep = format!("{}1{}", "[".repeat(1025), "]".repeat(1025));
        assert!(toon_value(&too_deep).is_err());
    }
}
