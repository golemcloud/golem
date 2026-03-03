// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    LBrace,
    RBrace,
    LBrack,
    RBrack,
    LParen,
    RParen,
    Comma,
    Colon,
    DoubleColon,
    Ident(String),
    StringLit(String),
    CharLit(char),
    IntLit(i64),
    UintLit(u64),
    FloatLit(f64),
    BoolLit(bool),
    Null,
    Undefined,
    AtT,
    AtB,
    Eof,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LexError {
    pub position: usize,
    pub message: String,
}

impl fmt::Display for LexError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Lex error at position {}: {}", self.position, self.message)
    }
}

impl std::error::Error for LexError {}

pub struct Lexer<'a> {
    input: &'a str,
    pos: usize,
    peeked: Option<(Token, usize, usize)>,
}

impl<'a> Lexer<'a> {
    pub fn new(input: &'a str) -> Lexer<'a> {
        Lexer {
            input,
            pos: 0,
            peeked: None,
        }
    }

    pub fn position(&self) -> usize {
        self.pos
    }

    pub fn skip_raw_char(&mut self, ch: u8) -> bool {
        if self.peeked.is_none() {
            self.skip_whitespace();
            if self.pos < self.input.len() && self.input.as_bytes()[self.pos] == ch {
                self.pos += 1;
                return true;
            }
        }
        false
    }

    pub fn peek(&mut self) -> Result<&Token, LexError> {
        if self.peeked.is_none() {
            let tok = self.read_token()?;
            self.peeked = Some(tok);
        }
        Ok(&self.peeked.as_ref().unwrap().0)
    }

    pub fn next_token(&mut self) -> Result<(Token, usize, usize), LexError> {
        if let Some(t) = self.peeked.take() {
            self.pos = t.2;
            return Ok(t);
        }
        self.read_token()
    }

    pub fn expect(&mut self, expected: &Token) -> Result<(usize, usize), LexError> {
        let (tok, start, end) = self.next_token()?;
        if tok == *expected {
            Ok((start, end))
        } else {
            Err(LexError {
                position: start,
                message: format!("expected {expected:?}, got {tok:?}"),
            })
        }
    }

    pub fn expect_ident(&mut self) -> Result<(String, usize, usize), LexError> {
        let (tok, start, end) = self.next_token()?;
        if let Token::Ident(s) = tok {
            Ok((s, start, end))
        } else {
            Err(LexError {
                position: start,
                message: format!("expected identifier, got {tok:?}"),
            })
        }
    }

    pub fn expect_string(&mut self) -> Result<(String, usize, usize), LexError> {
        let (tok, start, end) = self.next_token()?;
        if let Token::StringLit(s) = tok {
            Ok((s, start, end))
        } else {
            Err(LexError {
                position: start,
                message: format!("expected string literal, got {tok:?}"),
            })
        }
    }

    fn bytes(&self) -> &[u8] {
        self.input.as_bytes()
    }

    fn skip_whitespace(&mut self) {
        while self.pos < self.input.len() && matches!(self.bytes()[self.pos], b' ' | b'\t' | b'\n' | b'\r') {
            self.pos += 1;
        }
    }

    fn read_token(&mut self) -> Result<(Token, usize, usize), LexError> {
        self.skip_whitespace();
        let start = self.pos;
        let bytes = self.bytes();

        if start >= bytes.len() {
            return Ok((Token::Eof, start, start));
        }

        let ch = bytes[start];
        match ch {
            b'{' => { self.pos += 1; Ok((Token::LBrace, start, self.pos)) }
            b'}' => { self.pos += 1; Ok((Token::RBrace, start, self.pos)) }
            b'[' => { self.pos += 1; Ok((Token::LBrack, start, self.pos)) }
            b']' => { self.pos += 1; Ok((Token::RBrack, start, self.pos)) }
            b'(' => { self.pos += 1; Ok((Token::LParen, start, self.pos)) }
            b')' => { self.pos += 1; Ok((Token::RParen, start, self.pos)) }
            b',' => { self.pos += 1; Ok((Token::Comma, start, self.pos)) }
            b':' => {
                if self.pos + 1 < bytes.len() && bytes[self.pos + 1] == b':' {
                    self.pos += 2;
                    Ok((Token::DoubleColon, start, self.pos))
                } else {
                    self.pos += 1;
                    Ok((Token::Colon, start, self.pos))
                }
            }
            b'@' => {
                if self.pos + 1 < bytes.len() && bytes[self.pos + 1] == b't' {
                    self.pos += 2;
                    Ok((Token::AtT, start, self.pos))
                } else if self.pos + 1 < bytes.len() && bytes[self.pos + 1] == b'b' {
                    self.pos += 2;
                    Ok((Token::AtB, start, self.pos))
                } else {
                    Err(LexError { position: start, message: "unexpected character after '@'".into() })
                }
            }
            b'"' => self.read_string(start),
            b'\'' => self.read_char(start),
            b'-' => self.read_negative(start),
            b'0'..=b'9' => self.read_number(start, false),
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => self.read_ident(start),
            _ => Err(LexError { position: start, message: format!("unexpected character '{}'", ch as char) }),
        }
    }

    fn read_ident(&mut self, start: usize) -> Result<(Token, usize, usize), LexError> {
        while self.pos < self.input.len() && matches!(self.bytes()[self.pos], b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_') {
            self.pos += 1;
        }
        let word = &self.input[start..self.pos];
        let tok = match word {
            "true" => Token::BoolLit(true),
            "false" => Token::BoolLit(false),
            "null" => Token::Null,
            "undefined" => Token::Undefined,
            "NaN" => Token::FloatLit(f64::NAN),
            "Infinity" => Token::FloatLit(f64::INFINITY),
            _ => Token::Ident(word.to_string()),
        };
        Ok((tok, start, self.pos))
    }

    fn read_negative(&mut self, start: usize) -> Result<(Token, usize, usize), LexError> {
        self.pos += 1; // skip '-'
        if self.pos < self.input.len() {
            if self.bytes()[self.pos] == b'I' {
                let ident_start = self.pos;
                let (tok, _, end) = self.read_ident(ident_start)?;
                if let Token::FloatLit(v) = tok {
                    if v == f64::INFINITY {
                        return Ok((Token::FloatLit(f64::NEG_INFINITY), start, end));
                    }
                }
                Err(LexError { position: start, message: "expected number after '-'".into() })
            } else if self.bytes()[self.pos].is_ascii_digit() {
                self.read_number(start, true)
            } else {
                Err(LexError { position: start, message: "expected number after '-'".into() })
            }
        } else {
            Err(LexError { position: start, message: "unexpected end of input after '-'".into() })
        }
    }

    fn read_number(&mut self, start: usize, negative: bool) -> Result<(Token, usize, usize), LexError> {
        let digit_start = self.pos;
        while self.pos < self.input.len() && self.bytes()[self.pos].is_ascii_digit() {
            self.pos += 1;
        }
        let mut is_float = false;
        if self.pos < self.input.len() && self.bytes()[self.pos] == b'.' {
            is_float = true;
            self.pos += 1;
            while self.pos < self.input.len() && self.bytes()[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
        }
        if self.pos < self.input.len() && matches!(self.bytes()[self.pos], b'e' | b'E') {
            is_float = true;
            self.pos += 1;
            if self.pos < self.input.len() && matches!(self.bytes()[self.pos], b'+' | b'-') {
                self.pos += 1;
            }
            if self.pos >= self.input.len() || !self.bytes()[self.pos].is_ascii_digit() {
                return Err(LexError { position: digit_start, message: "invalid float exponent".into() });
            }
            while self.pos < self.input.len() && self.bytes()[self.pos].is_ascii_digit() {
                self.pos += 1;
            }
        }
        let text = &self.input[start..self.pos];
        if is_float {
            let v: f64 = text.parse().map_err(|e| LexError { position: start, message: format!("invalid float: {e}") })?;
            Ok((Token::FloatLit(v), start, self.pos))
        } else if negative {
            let v: i64 = text.parse().map_err(|e| LexError { position: start, message: format!("invalid integer: {e}") })?;
            Ok((Token::IntLit(v), start, self.pos))
        } else {
            let v: u64 = text.parse().map_err(|e| LexError { position: start, message: format!("invalid integer: {e}") })?;
            Ok((Token::UintLit(v), start, self.pos))
        }
    }

    fn read_string(&mut self, start: usize) -> Result<(Token, usize, usize), LexError> {
        self.pos += 1; // skip opening '"'
        let mut buf = String::new();
        loop {
            if self.pos >= self.input.len() {
                return Err(LexError { position: start, message: "unterminated string".into() });
            }
            match self.bytes()[self.pos] {
                b'"' => { self.pos += 1; return Ok((Token::StringLit(buf), start, self.pos)); }
                b'\\' => { buf.push(self.read_escape()?); }
                _ => {
                    let ch = self.current_char();
                    self.pos += ch.len_utf8();
                    buf.push(ch);
                }
            }
        }
    }

    fn read_char(&mut self, start: usize) -> Result<(Token, usize, usize), LexError> {
        self.pos += 1; // skip opening '\''
        if self.pos >= self.input.len() {
            return Err(LexError { position: start, message: "unterminated char literal".into() });
        }
        let ch = if self.bytes()[self.pos] == b'\\' {
            self.read_escape()?
        } else {
            let c = self.current_char();
            self.pos += c.len_utf8();
            c
        };
        if self.pos >= self.input.len() || self.bytes()[self.pos] != b'\'' {
            return Err(LexError { position: start, message: "unterminated char literal".into() });
        }
        self.pos += 1;
        Ok((Token::CharLit(ch), start, self.pos))
    }

    fn current_char(&self) -> char {
        self.input[self.pos..].chars().next().unwrap()
    }

    fn read_escape(&mut self) -> Result<char, LexError> {
        let esc_pos = self.pos;
        self.pos += 1; // skip '\\'
        if self.pos >= self.input.len() {
            return Err(LexError { position: esc_pos, message: "unterminated escape".into() });
        }
        let ch = self.bytes()[self.pos];
        self.pos += 1;
        match ch {
            b'"' => Ok('"'),
            b'\\' => Ok('\\'),
            b'/' => Ok('/'),
            b'n' => Ok('\n'),
            b't' => Ok('\t'),
            b'r' => Ok('\r'),
            b'b' => Ok('\u{08}'),
            b'f' => Ok('\u{0C}'),
            b'\'' => Ok('\''),
            b'u' => self.read_unicode_escape(esc_pos),
            _ => Err(LexError { position: esc_pos, message: format!("invalid escape '\\{}'", ch as char) }),
        }
    }

    fn read_unicode_escape(&mut self, esc_pos: usize) -> Result<char, LexError> {
        let code = self.read_hex4(esc_pos)?;
        if (0xD800..=0xDBFF).contains(&code) {
            // high surrogate — expect \uXXXX low surrogate
            if self.pos + 1 < self.input.len() && self.bytes()[self.pos] == b'\\' && self.bytes()[self.pos + 1] == b'u' {
                self.pos += 2;
                let low = self.read_hex4(esc_pos)?;
                if !(0xDC00..=0xDFFF).contains(&low) {
                    return Err(LexError { position: esc_pos, message: "invalid surrogate pair".into() });
                }
                let cp = 0x10000 + ((code as u32 - 0xD800) << 10) + (low as u32 - 0xDC00);
                char::from_u32(cp).ok_or_else(|| LexError { position: esc_pos, message: "invalid codepoint".into() })
            } else {
                Err(LexError { position: esc_pos, message: "expected low surrogate".into() })
            }
        } else {
            char::from_u32(code as u32).ok_or_else(|| LexError { position: esc_pos, message: "invalid codepoint".into() })
        }
    }

    fn read_hex4(&mut self, esc_pos: usize) -> Result<u16, LexError> {
        if self.pos + 4 > self.input.len() {
            return Err(LexError { position: esc_pos, message: "incomplete \\uXXXX escape".into() });
        }
        let hex = &self.input[self.pos..self.pos + 4];
        self.pos += 4;
        u16::from_str_radix(hex, 16).map_err(|_| LexError { position: esc_pos, message: format!("invalid hex in \\u{hex}") })
    }
}
