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
pub(all) struct LR0Item {
  production : @grm.Production
  dot : Int
} derive(Eq, Compare, Hash)

///|
pub fn LR0Item::new(production : @grm.Production, dot : Int) -> LR0Item {
  { production, dot }
}

///|
pub fn LR0Item::postdot(self : LR0Item) -> @grm.Symbol? {
  if self.dot < self.production.rhs.length() {
    Some(self.production.rhs[self.dot])
  } else {
    None
  }
}

///|
pub fn LR0Item::shift(self : LR0Item) -> LR0Item {
  guard self.dot < self.production.rhs.length()
  { ..self, dot: self.dot + 1 }
}
