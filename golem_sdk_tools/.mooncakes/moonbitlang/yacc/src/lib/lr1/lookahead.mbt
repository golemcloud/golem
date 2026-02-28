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
pub(all) enum Lookahead {
  EndOfInput
  Input(@grm.Terminal)
} derive(Eq, Compare)

///|
pub impl Hash for Lookahead with hash_combine(self, hasher) {
  match self {
    EndOfInput => hasher.combine_int(-1)
    Input(t) => hasher.combine_int(t.num)
  }
}

///|
pub impl Show for Lookahead with output(self, logger) {
  match self {
    Input(t) => logger.write_object(t)
    EndOfInput => logger.write_string("$")
  }
}
