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
suberror ParseOrLexError {
  ParseError(ParseError)
  LexError(LexError)
} derive(Show)

///|
pub fn parse(str : String) -> @ast.TypeExpr raise ParseOrLexError {
  let lexbuf = @lexbuf.StringLexbuf::from_string(str)
  let tokens = []
  while true {
    let triple = token(lexbuf) catch { err => raise LexError(err) }
    tokens.push(triple)
    if triple.0 is EOF {
      break
    }
  }
  parse_type_expr(tokens) catch {
    err => raise ParseError(err)
  }
}
