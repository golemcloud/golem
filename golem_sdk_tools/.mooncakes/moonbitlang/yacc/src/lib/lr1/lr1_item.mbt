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
pub(all) struct LR1Item {
  production : @grm.Production
  dot : Int
  lookahead : Lookahead
} derive(Eq, Compare)

///|
pub fn LR1Item::core(self : LR1Item) -> LR0Item {
  { production: self.production, dot: self.dot }
}

///|
pub fn LR1Item::postdot(self : LR1Item) -> @grm.Symbol? {
  if self.dot < self.production.rhs.length() {
    Some(self.production.rhs[self.dot])
  } else {
    None
  }
}

///|
pub fn LR1Item::shift(self : LR1Item) -> LR1Item {
  guard self.dot < self.production.rhs.length()
  { ..self, dot: self.dot + 1 }
}

///|
pub impl Show for LR1Item with output(self, logger) {
  logger.write_char('[')
  self.production.output_with_opt_dot(logger, dot=self.dot)
  logger.write_string(", ")
  logger.write_object(self.lookahead)
  logger.write_char(']')
}
