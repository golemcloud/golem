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
struct T[K, V] {
  mut bucket : FixedArray[Entry[K, V]?]
  mut entry : Entry[K, V]?
  mut bitmask : Int
  mut size : Int
}

///|
priv struct Entry[K, V] {
  mut value : V
  mut psl : Int
  key : K
  hash : Int
  prev : Entry[K, V]?
}

///|
/// Capacity must be a power of 2.
pub fn[K, V] new(capacity~ : Int) -> T[K, V] {
  let bucket = FixedArray::make(capacity, None)
  { bucket, entry: None, bitmask: capacity - 1, size: 0 }
}

///|
pub fn[K, V] T::size(self : T[K, V]) -> Int {
  self.size
}

// ///|
// pub fn[K, V] T::iter(self : T[K, V]) -> Iter[(K, V)] {
//   Iter::new(fn(yield_) {
//     loop self.entry {
//       None => IterContinue
//       Some(entry) =>
//         match yield_((entry.key, entry.value)) {
//           IterEnd => return IterEnd
//           IterContinue => continue entry.prev
//         }
//     }
//   })
// }

///|
pub fn[K, V] T::iter(self : T[K, V]) -> Iter[(K, V)] {
  let mut current_entry = self.entry
  Iter::new(fn() {
    if current_entry is Some(entry) {
      current_entry = entry.prev
      return Some((entry.key, entry.value))
    } else {
      return None
    }
  })
}

///|
// pub fn[K, V] T::iter2(self : T[K, V]) -> Iter2[K, V] {
//   Iter2::new(fn(yield_) {
//     loop self.entry {
//       None => IterContinue
//       Some(entry) =>
//         match yield_(entry.key, entry.value) {
//           IterEnd => return IterEnd
//           IterContinue => continue entry.prev
//         }
//     }
//   })
// }

pub fn[K, V] T::iterator2(self : T[K, V]) -> Iter2[K, V] {
  let mut current_entry = self.entry
  Iter2::new(fn() {
    if current_entry is Some(entry) {
      current_entry = entry.prev
      return Some((entry.key, entry.value))
    } else {
      return None
    }
  })
}

///|
pub fn[K : Hash + Eq, V] T::get(self : T[K, V], key : K) -> V? {
  match self.get_entry_with_hash(key, key.hash()) {
    Some(entry) => Some(entry.value)
    None => None
  }
}

///|
pub fn[K : Hash + Eq, V] T::contains(self : T[K, V], key : K) -> Bool {
  match self.get_entry_with_hash(key, key.hash()) {
    Some(_) => true
    None => false
  }
}

///|
fn[K : Eq, V] T::get_entry_with_hash(
  self : T[K, V],
  key : K,
  hash : Int,
) -> Entry[K, V]? {
  let bucket = self.bucket
  let bitmask = self.bitmask
  let ideal_index = hash & bitmask
  for index = ideal_index {
    match bucket[index] {
      None => break None
      Some(entry) => {
        if entry.hash == hash && entry.key == key {
          break Some(entry)
        }
        if index - ideal_index > entry.psl {
          break None
        }
        continue (index + 1) & bitmask
      }
    }
  }
}

///|
pub fn[K : Hash + Eq, V] T::get_or_init(
  self : T[K, V],
  key : K,
  init : (K) -> V,
) -> V {
  let hash = key.hash()
  match self.get_entry_with_hash(key, hash) {
    Some(entry) => entry.value
    None => {
      if self.size > self.bitmask / 2 {
        self.grow()
      }
      let value = init(key)
      let entry = { value, psl: 0, key, hash, prev: self.entry }
      self.entry = Some(entry)
      self.size += 1
      self.put_entry(hash & self.bitmask, self.bitmask, entry)
      value
    }
  }
}

///|
pub fn[K : Hash + Eq, V] T::set(self : T[K, V], key : K, value : V) -> Unit {
  let hash = key.hash()
  match self.get_entry_with_hash(key, hash) {
    Some(entry) => entry.value = value
    None => {
      if self.size > self.bitmask / 2 {
        self.grow()
      }
      let entry = { value, psl: 0, key, hash, prev: self.entry }
      self.entry = Some(entry)
      self.size += 1
      self.put_entry(hash & self.bitmask, self.bitmask, entry)
    }
  }
}

///|
fn[K, V] T::grow(self : T[K, V]) -> Unit {
  let capacity = (self.bitmask + 1) * 2
  let bitmask = capacity - 1
  let bucket = FixedArray::make(capacity, None)
  self.bucket = bucket
  self.bitmask = bitmask
  loop self.entry {
    None => ()
    Some(entry) => {
      entry.psl = 0
      self.put_entry(entry.hash & bitmask, bitmask, entry)
      continue entry.prev
    }
  }
}

///|
fn[K, V] T::put_entry(
  self : T[K, V],
  index : Int,
  bitmask : Int,
  entry : Entry[K, V],
) -> Unit {
  let bucket = self.bucket
  for index = index, entry = entry {
    match bucket[index] {
      None => {
        bucket[index] = Some(entry)
        break
      }
      Some(old_entry) => {
        let entry = if entry.psl > old_entry.psl {
          bucket[index] = Some(entry)
          old_entry
        } else {
          entry
        }
        entry.psl += 1
        continue (index + 1) & bitmask, entry
      }
    }
  }
}
