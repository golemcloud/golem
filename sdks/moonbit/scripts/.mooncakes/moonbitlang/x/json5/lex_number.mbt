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
fn lex_hexadecimal(ctx : ParseContext, neg~ : Bool) -> Double raise ParseError {
  match read_char(ctx) {
    Some(c) => {
      if c is ('0'..='9' | 'a'..='f' | 'A'..='F') {
        let n = lex_hexadecimal_integer(ctx, hex_digit_to_int(c))
        return if neg { -n } else { n }
      }
      ctx.offset -= 1
      invalid_char(ctx)
    }
    None => parse_error(InvalidEof)
  }
}

///|
fn lex_hexadecimal_integer(ctx : ParseContext, n : Int) -> Double {
  for n = n.to_double() {
    match read_char(ctx) {
      Some(c) => {
        if c is ('0'..='9' | 'a'..='f' | 'A'..='F') {
          continue n * 16.0 + hex_digit_to_int(c).to_double()
        }
        ctx.offset -= 1
        return n
      }
      None => return n
    }
  }
}

///|
fn lex_number_end(
  ctx : ParseContext,
  start : Int,
  end : Int,
) -> Double raise ParseError {
  let s = try! ctx.input[start:end].to_string()
  @strconv.parse_double(s) catch {
    _ => parse_error(InvalidNumber(offset_to_position(ctx.input, start), s))
  }
}
