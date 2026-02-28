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
struct EncodedLR1ItemSet {
  core : LR0Node
  kernel_lookahead_set_table : Array[EncodedLookaheadSet]
}

///|
fn EncodedLR1ItemSet::decode_item_groups(
  self : EncodedLR1ItemSet,
  grammar : @grm.Grammar,
) -> Iter[LR1ItemGroup] {
  let lookahead_set_array = self.core.closure_symbolic_lookahead_set.map(fn(
    symbolic_lookahead_set,
  ) {
    symbolic_lookahead_set.interpret(self.kernel_lookahead_set_table)
  })
  self.core.closure_items
  .decode_iter(grammar)
  .map(item => {
    let (index, item) = item
    let lookahead_set = lookahead_set_array[index]
    LR1ItemGroup::{
      core: item,
      lookahead_set: lookahead_set.decode_iter(grammar),
    }
  })
}

///|
fn EncodedLR1ItemSet::merge(
  self : EncodedLR1ItemSet,
  other : EncodedLR1ItemSet,
) -> EncodedLR1ItemSet {
  guard self.core == other.core
  {
    core: self.core,
    kernel_lookahead_set_table: self.kernel_lookahead_set_table.mapi(fn(
      i,
      lookahead_set,
    ) {
      lookahead_set.union(other.kernel_lookahead_set_table[i])
    }),
  }
}

///|
fn EncodedLR1ItemSet::subsume(
  self : EncodedLR1ItemSet,
  other : EncodedLR1ItemSet,
) -> Bool {
  guard self.core == other.core
  let len = self.kernel_lookahead_set_table.length()
  let self_table = self.kernel_lookahead_set_table
  let other_table = other.kernel_lookahead_set_table
  match len {
    1 => self_table[0].subset(other_table[0])
    2 =>
      self_table[0].subset(other_table[0]) &&
      self_table[1].subset(other_table[1])
    _ => {
      for i in 0..<len {
        if not(self_table[i].subset(other_table[i])) {
          return false
        }
      }
      return true
    }
  }
}

///|
fn EncodedLR1ItemSet::weak_compat(
  self : EncodedLR1ItemSet,
  other : EncodedLR1ItemSet,
) -> Bool {
  guard self.core == other.core
  let len = self.kernel_lookahead_set_table.length()
  for i in 0..<len {
    let la_set1i = self.kernel_lookahead_set_table[i]
    let la_set2i = other.kernel_lookahead_set_table[i]
    for j in i..<len {
      guard i != j else { continue }
      let la_set1j = self.kernel_lookahead_set_table[j]
      let la_set2j = other.kernel_lookahead_set_table[j]
      let compat = (la_set1i.disjoint(la_set2j) && la_set2i.disjoint(la_set1j)) ||
        not(la_set1i.disjoint(la_set1j)) ||
        not(la_set2i.disjoint(la_set2j))
      if not(compat) {
        return false
      }
    }
  }
  true
}

///|
fn EncodedLR1ItemSet::eoi_compact(
  self : EncodedLR1ItemSet,
  other : EncodedLR1ItemSet,
) -> Bool {
  guard self.core == other.core
  let len = self.kernel_lookahead_set_table.length()
  for i in 0..<len {
    let la_set1 = self.kernel_lookahead_set_table[i]
    let la_set2 = other.kernel_lookahead_set_table[i]
    if la_set1.is_singleton(EndOfInput) {
      if not(la_set2.is_singleton(EndOfInput)) {
        return false
      }
    } else if la_set2.is_singleton(EndOfInput) {
      if not(la_set1.is_singleton(EndOfInput)) {
        return false
      }
    }
  }
  true
}
