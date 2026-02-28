// Copyright (C) 2025 International Digital Economy Academy
//
// This program is free software; you can redistribute it and/or
// modify it under the terms of the GNU General Public License
// as published by the Free Software Foundation; version 2.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program; if not, see <https://www.gnu.org/licenses/>.

///|
test {
  footer()
  let tokens = [
    NUMBER(3),
    PLUS,
    LPAREN,
    NUMBER(2),
    MINUS,
    NUMBER(5),
    RPAREN,
    STAR,
    NUMBER(0),
  ]
  let mut cursor = 0
  fn read_token() -> (Token, Int, Int) {
    guard cursor < tokens.length() else { (EOF, cursor, cursor) }
    let token = tokens[cursor]
    cursor += 1
    (token, cursor - 1, cursor)
  }

  let result = start(read_token, 0)
  assert_eq(result, 3)
}

///|
test {
  let tokens = [NUMBER(3), PLUS, LPAREN, NUMBER(2), MINUS, NUMBER(5)]
  let mut cursor = 0
  fn read_token() -> (Token, Int, Int) {
    guard cursor < tokens.length() else { (EOF, cursor, cursor) }
    let token = tokens[cursor]
    cursor += 1
    (token, cursor - 1, cursor)
  }

  let err : ParseError? = try start(read_token, 0) catch {
    err => Some(err)
  } noraise {
    _ => None
  }
  inspect(
    err,
    content=(
      #|Some(UnexpectedToken(EOF, (6, 6), ["+", "-", "*", ")"]))
    ),
  )
}
