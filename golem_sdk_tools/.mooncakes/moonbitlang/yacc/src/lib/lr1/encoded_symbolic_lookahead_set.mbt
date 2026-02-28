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
priv struct EncodedSymbolicLookaheadSet {
  concrete_set : EncodedLookaheadSet
  vars : @small_int_set.T
}

///|
let _empty : EncodedSymbolicLookaheadSet = {
  concrete_set: EncodedLookaheadSet::empty(),
  vars: @small_int_set.empty(),
}

///|
impl Eq for EncodedSymbolicLookaheadSet with equal(self, other) {
  self.concrete_set == other.concrete_set && self.vars == other.vars
}

///|
impl AbstractLookaheadSet for EncodedSymbolicLookaheadSet with empty() {
  _empty
}

///|
impl AbstractLookaheadSet for EncodedSymbolicLookaheadSet with singleton(
  lookahead,
) {
  {
    concrete_set: EncodedLookaheadSet::singleton(lookahead),
    vars: @small_int_set.empty(),
  }
}

///|
fn EncodedSymbolicLookaheadSet::variable(
  var_ : Int,
) -> EncodedSymbolicLookaheadSet {
  {
    concrete_set: EncodedLookaheadSet::empty(),
    vars: @small_int_set.singleton(var_),
  }
}

///|
impl AbstractLookaheadSet for EncodedSymbolicLookaheadSet with union(
  self,
  other,
) {
  {
    concrete_set: self.concrete_set.union(other.concrete_set),
    vars: self.vars.union(other.vars),
  }
}

///|
fn EncodedSymbolicLookaheadSet::interpret(
  self : EncodedSymbolicLookaheadSet,
  subst : Array[EncodedLookaheadSet],
) -> EncodedLookaheadSet {
  self.vars
  .iter()
  .map(fn(var_) { subst[var_] })
  .fold(init=self.concrete_set, EncodedLookaheadSet::union)
}
