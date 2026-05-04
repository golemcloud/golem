// Copyright 2025 International Digital Economy Academy
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

///|
fn read_char(ctx : ParseContext) -> Char? {
  if ctx.offset < ctx.end_offset {
    let c = ctx.input.code_unit_at(ctx.offset).to_int()
    ctx.offset += 1
    let c1 = c
    if c1 >= 0xD800 && c1 <= 0xDBFF {
      if ctx.offset < ctx.end_offset {
        let c2 = ctx.input.code_unit_at(ctx.offset).to_int()
        if c2 >= 0xDC00 && c2 <= 0xDFFF {
          ctx.offset += 1
          let c3 = (c1 << 10) + c2 - 0x35fdc00
          return Some(Int::unsafe_to_char(c3))
        }
      }
    }
    Some(Int::unsafe_to_char(c))
  } else {
    None
  }
}

///|
fn lex_skip_whitespace(ctx : ParseContext) -> Unit raise ParseError {
  for {
    match read_char(ctx) {
      Some('\t' | '\u000B' | '\u000C' | ' ' | '\n' | '\r') => continue
      Some('/') => {
        lex_comment(ctx)
        continue
      }
      Some(c) => {
        if c > '\u{7f}' && non_ascii_whitespace.contains(c) {
          continue
        }
        ctx.offset -= 1
        return ()
      }
      None => return ()
    }
  }
}

///|
fn lex_after_array_value(ctx : ParseContext) -> Token raise ParseError {
  lex_skip_whitespace(ctx)
  match read_char(ctx) {
    Some(']') => Token::RBracket
    Some(',') => Token::Comma
    Some(_) => invalid_char(ctx, shift=-1)
    None => parse_error(InvalidEof)
  }
}

///|
fn lex_after_property_name(ctx : ParseContext) -> Token raise ParseError {
  lex_skip_whitespace(ctx)
  match read_char(ctx) {
    Some(':') => Token::Colon
    Some(_) => invalid_char(ctx, shift=-1)
    None => parse_error(InvalidEof)
  }
}

///|
fn lex_after_object_value(ctx : ParseContext) -> Token raise ParseError {
  lex_skip_whitespace(ctx)
  match read_char(ctx) {
    Some('}') => Token::RBrace
    Some(',') => Token::Comma
    Some(_) => invalid_char(ctx, shift=-1)
    None => parse_error(InvalidEof)
  }
}

///|
fn lex_assert_char(ctx : ParseContext, c : Char) -> Unit raise ParseError {
  match read_char(ctx) {
    Some(c2) => if c == c2 { () } else { invalid_char(ctx, shift=-1) }
    None => parse_error(InvalidEof)
  }
}

///|
fn lex_comment(ctx : ParseContext) -> Unit raise ParseError {
  match read_char(ctx) {
    Some('/') =>
      for {
        match read_char(ctx) {
          Some('\n' | '\r' | '\u2028' | '\u2029') => {
            ctx.offset -= 1
            return ()
          }
          None => return ()
          _ => ()
        }
      }
    Some('*') =>
      for {
        match read_char(ctx) {
          Some('*') =>
            match read_char(ctx) {
              Some('/') => return ()
              _ => ()
            }
          None => parse_error(InvalidEof)
          _ => ()
        }
      }
    Some(_) => invalid_char(ctx, shift=-1)
    None => parse_error(InvalidEof)
  }
}
