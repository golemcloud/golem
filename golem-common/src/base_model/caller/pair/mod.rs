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

use serde::{Deserialize, Serialize};
use std::fmt::{Display, Formatter, Write};
use std::str::FromStr;

/// The issuer of GitHub sign-ins. GitHub sign-in is OAuth and sends no issuer, so every
/// installation uses this one string, with the numeric GitHub user ID as the subject.
pub const GITHUB_ISSUER: &str = "https://github.com";

/// One identity: an issuer and a subject.
///
/// Two pairs are equal exactly when both strings are equal byte for byte. Nothing folds case or
/// normalizes a trailing slash or Unicode text: OpenID Connect compares both as exact strings,
/// so a normalization would merge two users or split one.
///
/// The text form, from [`Display`] and [`FromStr`], is `("<issuer>","<subject>")` with no spaces.
/// Each string is escaped as RFC 8785 serializes JSON strings, and the parser refuses every other
/// spelling, so two texts are equal exactly when their pairs are equal.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Pair {
    pub issuer: String,
    pub subject: String,
}

impl Pair {
    pub fn new(issuer: impl Into<String>, subject: impl Into<String>) -> Self {
        Self {
            issuer: issuer.into(),
            subject: subject.into(),
        }
    }

    /// The pair of a GitHub user. The subject is the numeric user ID, never the login name, so a
    /// renamed GitHub account keeps its pair.
    pub fn github_user(user_id: u64) -> Self {
        Self::new(GITHUB_ISSUER, user_id.to_string())
    }
}

impl Display for Pair {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_char('(')?;
        write_string(f, &self.issuer)?;
        f.write_char(',')?;
        write_string(f, &self.subject)?;
        f.write_char(')')
    }
}

/// Writes `value` as a quoted string, escaped as RFC 8785 (section 3.2.2.2) serializes JSON
/// strings: `\"` and `\\`; `\b`, `\t`, `\n`, `\f` and `\r`; every other control character as
/// `\u00xx` in lowercase hex; every other character, `/` and non-ASCII text included, as it is.
fn write_string(f: &mut Formatter<'_>, value: &str) -> std::fmt::Result {
    f.write_char('"')?;
    for character in value.chars() {
        match character {
            '"' => f.write_str("\\\"")?,
            '\\' => f.write_str("\\\\")?,
            '\u{08}' => f.write_str("\\b")?,
            '\t' => f.write_str("\\t")?,
            '\n' => f.write_str("\\n")?,
            '\u{0c}' => f.write_str("\\f")?,
            '\r' => f.write_str("\\r")?,
            control if control < '\u{20}' => write!(f, "\\u{:04x}", control as u32)?,
            other => f.write_char(other)?,
        }
    }
    f.write_char('"')
}

impl FromStr for Pair {
    type Err = TextFormError;

    /// Reads the canonical text form, and refuses every other spelling of a pair.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let mut parser = Parser { text, position: 0 };
        parser.expect('(', "`(`")?;
        let issuer = parser.string()?;
        parser.expect(',', "`,`")?;
        let subject = parser.string()?;
        parser.expect(')', "`)`")?;
        if parser.position < text.len() {
            return Err(TextFormError::TrailingText {
                position: parser.position,
            });
        }
        Ok(Pair { issuer, subject })
    }
}

struct Parser<'a> {
    text: &'a str,
    position: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.text[self.position..].chars().next()
    }

    fn expect(&mut self, expected: char, name: &'static str) -> Result<(), TextFormError> {
        if self.peek() == Some(expected) {
            self.position += expected.len_utf8();
            Ok(())
        } else {
            Err(TextFormError::Expected {
                expected: name,
                position: self.position,
            })
        }
    }

    fn string(&mut self) -> Result<String, TextFormError> {
        let start = self.position;
        self.expect('"', "`\"`")?;
        let mut value = String::new();
        loop {
            let at = self.position;
            let character = self
                .peek()
                .ok_or(TextFormError::UnterminatedString { position: start })?;
            self.position += character.len_utf8();
            match character {
                '"' => return Ok(value),
                '\\' => value.push(self.escape(at)?),
                control if control < '\u{20}' => {
                    return Err(TextFormError::UnescapedControlCharacter { position: at });
                }
                other => value.push(other),
            }
        }
    }

    /// Reads the escape after a backslash at `at`. Only the escapes that the printer writes are
    /// accepted, so each character has one spelling.
    fn escape(&mut self, at: usize) -> Result<char, TextFormError> {
        let non_canonical = TextFormError::NonCanonicalEscape { position: at };
        let character = self.peek().ok_or(non_canonical.clone())?;
        self.position += character.len_utf8();
        match character {
            '"' => Ok('"'),
            '\\' => Ok('\\'),
            'b' => Ok('\u{08}'),
            't' => Ok('\t'),
            'n' => Ok('\n'),
            'f' => Ok('\u{0c}'),
            'r' => Ok('\r'),
            'u' => {
                let digits = self
                    .text
                    .get(self.position..self.position + 4)
                    .ok_or(non_canonical.clone())?;
                if !digits
                    .bytes()
                    .all(|digit| matches!(digit, b'0'..=b'9' | b'a'..=b'f'))
                {
                    return Err(non_canonical);
                }
                let code = u32::from_str_radix(digits, 16).map_err(|_| non_canonical.clone())?;
                let has_short_escape = matches!(code, 0x08 | 0x09 | 0x0a | 0x0c | 0x0d);
                if code >= 0x20 || has_short_escape {
                    return Err(non_canonical);
                }
                self.position += 4;
                char::from_u32(code).ok_or(non_canonical)
            }
            _ => Err(non_canonical),
        }
    }
}

/// A stored pair. It is data only: holding a reference grants nothing, and never makes the
/// holder a caller. An anonymous principal has no pair, so it has no reference.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PrincipalRef(Pair);

impl PrincipalRef {
    pub fn new(pair: Pair) -> Self {
        Self(pair)
    }

    pub fn pair(&self) -> &Pair {
        &self.0
    }

    pub fn into_pair(self) -> Pair {
        self.0
    }
}

impl From<Pair> for PrincipalRef {
    fn from(pair: Pair) -> Self {
        Self(pair)
    }
}

impl Display for PrincipalRef {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for PrincipalRef {
    type Err = TextFormError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Pair::from_str(text).map(Self)
    }
}

/// Why a text is not the canonical text form of a pair. Each position is a byte offset.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TextFormError {
    #[error("expected {expected} at byte {position}")]
    Expected {
        expected: &'static str,
        position: usize,
    },
    #[error("the string that starts at byte {position} has no closing quote")]
    UnterminatedString { position: usize },
    #[error("the escape at byte {position} is not the canonical spelling of its character")]
    NonCanonicalEscape { position: usize },
    #[error("the control character at byte {position} must be escaped")]
    UnescapedControlCharacter { position: usize },
    #[error("unexpected text after the pair at byte {position}")]
    TrailingText { position: usize },
}

#[cfg(test)]
mod tests;
