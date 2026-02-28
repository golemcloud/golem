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
/// 0: EndOfInput
/// 1..<1024: Terminal number + 1
priv struct EncodedLookaheadSet(@small_int_set.T) derive(Eq)

///|
fn EncodedLookaheadSet::is_singleton(
  self : EncodedLookaheadSet,
  key : Lookahead,
) -> Bool {
  self.0.is_singleton(
    match key {
      EndOfInput => 0
      Input(key) => key.num + 1
    },
  )
}

///|
fn EncodedLookaheadSet::empty() -> EncodedLookaheadSet {
  @small_int_set.empty()
}

///|
fn EncodedLookaheadSet::terminal(t : @grm.Terminal) -> EncodedLookaheadSet {
  @small_int_set.singleton(t.num + 1)
}

///|
fn EncodedLookaheadSet::end_of_input() -> EncodedLookaheadSet {
  @small_int_set.singleton(0)
}

///|
fn EncodedLookaheadSet::singleton(input : Lookahead) -> EncodedLookaheadSet {
  match input {
    Input(t) => EncodedLookaheadSet::terminal(t)
    EndOfInput => EncodedLookaheadSet::end_of_input()
  }
}

///|
fn EncodedLookaheadSet::decode_iter(
  self : EncodedLookaheadSet,
  grammar : @grm.Grammar,
) -> Iter[Lookahead] {
  self.0
  .iter()
  .map(fn(t) {
    if t == 0 {
      EndOfInput
    } else {
      Input(grammar.terminals[t - 1])
    }
  })
}

///|
fn EncodedLookaheadSet::union(
  self : EncodedLookaheadSet,
  other : EncodedLookaheadSet,
) -> EncodedLookaheadSet {
  self.0.union(other.0)
}

///|
fn EncodedLookaheadSet::disjoint(
  self : EncodedLookaheadSet,
  other : EncodedLookaheadSet,
) -> Bool {
  self.0.disjoint(other.0)
}

///|
fn EncodedLookaheadSet::subset(
  self : EncodedLookaheadSet,
  other : EncodedLookaheadSet,
) -> Bool {
  self.0.subset(other.0)
}
