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
    (NUMBER(3), 0, 1),
    (PLUS, 1, 2),
    (LPAREN, 2, 3),
    (NUMBER(2), 3, 4),
    (MINUS, 4, 5),
    (NUMBER(5), 5, 6),
    (RPAREN, 6, 7),
    (STAR, 7, 8),
    (NUMBER(0), 8, 9),
  ]
  let result = start(tokens, initial_pos=0)
  assert_eq(result, 3)
}
