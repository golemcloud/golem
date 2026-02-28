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
pub(all) struct EncodedLR0Item(Int) derive(Eq, Compare, Hash)

///|
pub fn EncodedLR0Item::new(
  production : @grm.Production,
  dot : Int,
) -> EncodedLR0Item {
  (production.num << 10) | dot
}

///|
pub fn EncodedLR0Item::decode(
  self : EncodedLR0Item,
  grammar : @grm.Grammar,
) -> LR0Item {
  let production = grammar.productions[self.0 >> 10]
  let dot = self.0 & 0x3FF
  { production, dot }
}

///|
pub fn EncodedLR0Item::decode_production(
  self : EncodedLR0Item,
  grammar : @grm.Grammar,
) -> @grm.Production {
  grammar.productions[self.0 >> 10]
}

///|
pub fn EncodedLR0Item::decode_postdot(
  self : EncodedLR0Item,
  grammar : @grm.Grammar,
) -> @grm.Symbol? {
  let production = grammar.productions[self.0 >> 10]
  let dot = self.0 & 0x3FF
  production.rhs.get(dot)
}

///|
pub fn EncodedLR0Item::unsafe_shift(self : EncodedLR0Item) -> EncodedLR0Item {
  self.0 + 1
}
