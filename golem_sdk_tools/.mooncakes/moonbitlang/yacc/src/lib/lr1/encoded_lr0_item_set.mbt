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
/// Always sorted
pub(all) struct EncodedLR0ItemSet {
  items : Array[EncodedLR0Item]
  mut hash : Int?
}

///|
pub impl Eq for EncodedLR0ItemSet with equal(self, other) {
  self.items == other.items
}

///|
pub impl Hash for EncodedLR0ItemSet with hash_combine(self, _hasher) {
  panic()
  ignore(self)
}

///|
pub impl Hash for EncodedLR0ItemSet with hash(self) {
  match self.hash {
    Some(hash) => hash
    None => {
      let hasher = Hasher::new()
      for item in self.items {
        hasher.combine_int(item.0)
      }
      let hash = hasher.finalize()
      self.hash = Some(hash)
      hash
    }
  }
}

///|
pub fn EncodedLR0ItemSet::empty() -> EncodedLR0ItemSet {
  { items: [], hash: None }
}

///|
pub impl Default for EncodedLR0ItemSet with default() {
  EncodedLR0ItemSet::empty()
}

///|
pub fn EncodedLR0ItemSet::from_sorted_array(
  items : Array[EncodedLR0Item],
) -> EncodedLR0ItemSet {
  { items, hash: None }
}

///|
pub fn EncodedLR0ItemSet::decode_iter(
  self : EncodedLR0ItemSet,
  grammar : @grm.Grammar,
) -> Iter[(Int, LR0Item)] {
  self.items.iter().mapi((index, item) => (index, item.decode(grammar)))
}
