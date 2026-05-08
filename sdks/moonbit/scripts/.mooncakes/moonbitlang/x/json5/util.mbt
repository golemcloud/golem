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
fn offset_to_position(input : String, offset : Int) -> Position {
  let mut line = 1
  let mut column = 0
  for i = 0; i < offset; i = i + 1 {
    let c = input.code_unit_at(i)
    if c == '\n' {
      line += 1
      column = 0
    } else {
      column += 1
    }
  }
  return Position::{ line, column }
}

///|
fn[X] no_valid_token(
  ctx : ParseContext,
  shift? : Int = 0,
) -> X raise ParseError {
  let offset = ctx.offset + shift
  parse_error(NoValidToken(offset_to_position(ctx.input, offset)))
}

///|
fn[X] invalid_char(ctx : ParseContext, shift? : Int = 0) -> X raise ParseError {
  let offset = ctx.offset + shift
  parse_error(
    InvalidChar(
      offset_to_position(ctx.input, offset),
      Int::unsafe_to_char(ctx.input.code_unit_at(offset).to_int()),
    ),
  )
}

///|
fn hex_digit_to_int(c : Char) -> Int {
  if c >= 'A' {
    (c.to_int() & (32).lnot()) - 'A'.to_int() + 10
  } else {
    c.to_int() - '0'.to_int()
  }
}
