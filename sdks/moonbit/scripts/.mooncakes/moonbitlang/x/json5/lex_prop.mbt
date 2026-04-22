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
fn lex_property_name(ctx : ParseContext) -> Token raise ParseError {
  lex_skip_whitespace(ctx)
  match read_char(ctx) {
    Some('}') => RBrace
    Some('\'' | '"' as c) => {
      let s = lex_string(ctx, c)
      String(s)
    }
    Some('$' | '_') => {
      let s = lex_ident(ctx, ctx.offset - 1)
      String(s)
    }
    Some('\\') => {
      lex_assert_char(ctx, 'u')
      let c = lex_hex_digits(ctx, 4)
      let c = Int::unsafe_to_char(c)
      if c is ('$' | '_' | 'a'..='z' | 'A'..='Z' | '0'..='9') ||
        (c > '\u{7f}' && non_ascii_id_start.contains(c)) {
        let buffer = StringBuilder::new()
        buffer.write_char(c)
        let s = lex_ident(ctx, ctx.offset, buffer~)
        String(s)
      } else {
        parse_error(
          InvalidIdentEscape(offset_to_position(ctx.input, ctx.offset - 6)),
        )
      }
    }
    Some(c) => {
      if c is ('a'..='z' | 'A'..='Z') ||
        (c > '\u{7f}' && non_ascii_id_start.contains(c)) {
        let s = lex_ident(ctx, ctx.offset - 1)
        return String(s)
      }
      invalid_char(ctx, shift=-1)
    }
    None => parse_error(InvalidEof)
  }
}

///|
fn lex_ident(
  ctx : ParseContext,
  start : Int,
  buffer? : StringBuilder = StringBuilder::new(),
) -> String raise ParseError {
  let mut start = start
  fn flush(end : Int) {
    if start > 0 && end > start {
      buffer.write_view(ctx.input[start:end]) catch {
        _ => () // If slicing fails, just skip writing
      }
    }
  }

  for {
    match read_char(ctx) {
      Some('\\') => {
        flush(ctx.offset - 1)
        lex_assert_char(ctx, 'u')
        let c = lex_hex_digits(ctx, 4)
        let c = Int::unsafe_to_char(c)
        if c is ('$' | '_' | 'a'..='z' | 'A'..='Z' | '0'..='9') ||
          (c > '\u{7f}' && non_ascii_id_continue.contains(c)) {
          buffer.write_char(c)
          start = ctx.offset
          continue
        } else {
          parse_error(
            InvalidIdentEscape(offset_to_position(ctx.input, ctx.offset - 6)),
          )
        }
      }
      Some('$' | '_') => continue
      Some(c) => {
        if c is ('a'..='z' | 'A'..='Z' | '0'..='9') ||
          (c > '\u{7f}' && non_ascii_id_continue.contains(c)) {
          continue
        }
        ctx.offset -= 1
        break
      }
      None => break
    }
  }
  flush(ctx.offset)
  buffer.to_string()
}
