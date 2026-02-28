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
  let tokens = [
    (LBRACE, (), ()),
    (LETTER('a'), (), ()),
    (LETTER('b'), (), ()),
    (LETTER('c'), (), ()),
    (RBRACE, (), ()),
  ]
  let result = start(tokens, initial_pos=())
  assert_eq(result, "abc")
}
