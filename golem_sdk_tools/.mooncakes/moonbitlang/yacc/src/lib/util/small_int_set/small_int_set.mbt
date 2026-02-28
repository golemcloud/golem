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
struct SmallIntSet(Array[Int]) derive(Eq, Compare)

///|
pub type T = SmallIntSet

///|
let _empty : Array[Int] = [0]

///|
pub fn empty() -> SmallIntSet {
  _empty
}

///|
pub fn singleton(key : Int) -> SmallIntSet {
  let word_index = key >> 5
  let result = Array::make(2, 0)
  result[0] = 1 << word_index
  result[1] = 1 << (key & 31)
  result
}

///|
pub fn SmallIntSet::is_empty(self : SmallIntSet) -> Bool {
  self.0.length() == 1
}

///|
pub fn SmallIntSet::is_singleton(self : SmallIntSet, key : Int) -> Bool {
  self.0.length() == 2 &&
  self.0[0] == 1 << (key >> 5) &&
  self.0[1] == 1 << (key & 31)
}

///|
pub impl Hash for SmallIntSet with hash_combine(self, hasher) {
  for x in self.0 {
    hasher.combine_int(x)
  }
}

///|
pub impl Show for SmallIntSet with output(self, logger) {
  logger.write_string("{")
  let mut first = true
  for key in self {
    if first {
      first = false
    } else {
      logger.write_string(", ")
    }
    logger.write_object(key)
  }
  logger.write_string("}")
}

///|
// pub fn SmallIntSet::iter(self : SmallIntSet) -> Iter[Int] {
//   Iter::new(fn(yield_) {
//     let mut i = 0
//     let mut bitmask = self.0[0]
//     while true {
//       if bitmask == 0 {
//         break
//       }
//       let word_index = bitmask.ctz()
//       let mut word = self.0[1 + i]
//       while true {
//         if word == 0 {
//           break
//         }
//         let bit = word.ctz()
//         match yield_(word_index * 32 + bit) {
//           IterEnd => return IterEnd
//           IterContinue => ()
//         }
//         word = word & (1 << bit).lnot()
//       }
//       bitmask = bitmask & (1 << word_index).lnot()
//       i += 1
//     }
//     IterContinue
//   })
// }

pub fn SmallIntSet::iter(self : SmallIntSet) -> Iter[Int] {
  let mut i = 0
  let mut bitmask = self.0[0]
  // handle empty input
  if bitmask == 0 {
    return Iter::empty()
  }
  let mut word = self.0[1 + i]
  Iter::new(fn() {
    if bitmask == 0 {
      return None
    }
    let mut word_index = bitmask.ctz()
    if word == 0 {
      bitmask = bitmask & (1 << word_index).lnot()
      i += 1
      if bitmask == 0 {
        return None
      }
      word_index = bitmask.ctz()
      word = self.0[1 + i]
    }
    let bit = word.ctz()
    word = word & (1 << bit).lnot()
    return Some(word_index * 32 + bit)
  })
}

///|
// pub fn from_iter(iter : Iter[Int]) -> SmallIntSet {
//   let mut bitmask = 0
//   for key in iter {
//     let word_index = key >> 5
//     bitmask = bitmask | (1 << word_index)
//   }
//   let result = Array::make(1 + bitmask.popcnt(), 0)
//   result[0] = bitmask
//   for key in iter {
//     let word_index = key >> 5
//     let data_index = 1 + (bitmask & ((1 << word_index) - 1)).popcnt()
//     result[data_index] = result[data_index] | (1 << (key & 31))
//   }
//   result
// }

pub fn from_iter(iter : Iter[Int]) -> SmallIntSet {
  let data = iter.collect()
  let mut bitmask = 0
  for key in data {
    let word_index = key >> 5
    bitmask = bitmask | (1 << word_index)
  }
  let result = Array::make(1 + bitmask.popcnt(), 0)
  result[0] = bitmask
  for key in data {
    let word_index = key >> 5
    let data_index = 1 + (bitmask & ((1 << word_index) - 1)).popcnt()
    result[data_index] = result[data_index] | (1 << (key & 31))
  }
  result
}

///|
pub fn SmallIntSet::contains(self : SmallIntSet, key : Int) -> Bool {
  let word_index = key >> 5
  let bitmask = self.0[0]
  if (bitmask & (1 << word_index)) == 0 {
    return false
  }
  let data_index = 1 + (bitmask & ((1 << word_index) - 1)).popcnt()
  (self.0[data_index] & (1 << (key & 31))) != 0
}

///|
pub fn SmallIntSet::add(self : SmallIntSet, key : Int) -> SmallIntSet {
  let word_index = key >> 5
  let bitmask0 = self.0[0]
  let bitmask = bitmask0 | (1 << word_index)
  let data_index = 1 + (bitmask & ((1 << word_index) - 1)).popcnt()
  if bitmask == bitmask0 {
    if (self.0[data_index] & (1 << (key & 31))) != 0 {
      return self
    }
    let result = self.0.copy()
    result[data_index] = result[data_index] | (1 << (key & 31))
    result
  } else {
    let result = Array::make(1 + bitmask.popcnt(), 0)
    result[0] = bitmask
    self.0[1:data_index].blit_to(result, dst_offset=1)
    self.0[data_index:].blit_to(result, dst_offset=data_index + 1)
    result[data_index] = 1 << (key & 31)
    result
  }
}

///|
pub fn SmallIntSet::remove(self : SmallIntSet, key : Int) -> SmallIntSet {
  let word_index = key >> 5
  let bitmask = self.0[0]
  if (bitmask & (1 << word_index)) == 0 {
    return self
  }
  let result = self.0.copy()
  let data_index = 1 + (bitmask & ((1 << word_index) - 1)).popcnt()
  result[data_index] = self.0[data_index] & (1 << (key & 31)).lnot()
  if result[data_index] == 0 {
    result[0] = bitmask & (1 << word_index).lnot()
    result.remove(data_index) |> ignore
  }
  result
}

///|
pub fn SmallIntSet::union(
  self : SmallIntSet,
  other : SmallIntSet,
) -> SmallIntSet {
  let bitmask1 = self.0[0]
  if bitmask1 == 0 {
    return other
  }
  let bitmask2 = other.0[0]
  if bitmask2 == 0 {
    return self
  }
  let bitmask = bitmask1 | bitmask2
  if bitmask == bitmask1 {
    if physical_equal(self, other) {
      return self
    }
    let result = self.0.copy()
    let mut bitmask2 = bitmask2
    let mut j = 1
    while true {
      let jb = bitmask2.ctz()
      if jb == 32 {
        break
      }
      let i = 1 + (bitmask1 & ((1 << jb) - 1)).popcnt()
      result[i] = result[i] | other.0[j]
      j += 1
      bitmask2 = bitmask2 & (bitmask2 - 1)
    }
    result
  } else if bitmask == bitmask2 {
    if physical_equal(self, other) {
      return self
    }
    let result = other.0.copy()
    let mut bitmask1 = bitmask1
    let mut i = 1
    while true {
      let ib = bitmask1.ctz()
      if ib == 32 {
        break
      }
      let j = 1 + (bitmask2 & ((1 << ib) - 1)).popcnt()
      result[j] = result[j] | self.0[i]
      i += 1
      bitmask1 = bitmask1 & (bitmask1 - 1)
    }
    result
  } else {
    let result = Array::make(1 + bitmask.popcnt(), 0)
    result[0] = bitmask
    let mut i = 1
    let mut j = 1
    let mut k = 1
    let mut bitmask1 = bitmask1
    let mut bitmask2 = bitmask2
    while true {
      let ib = bitmask1.ctz()
      let jb = bitmask2.ctz()
      if ib < jb {
        result[k] = self.0[i]
        i += 1
        bitmask1 = bitmask1 & (bitmask1 - 1)
      } else if ib > jb {
        result[k] = other.0[j]
        j += 1
        bitmask2 = bitmask2 & (bitmask2 - 1)
      } else {
        if ib == 32 {
          break
        }
        result[k] = self.0[i] | other.0[j]
        i += 1
        j += 1
        bitmask1 = bitmask1 & (bitmask1 - 1)
        bitmask2 = bitmask2 & (bitmask2 - 1)
      }
      k += 1
    }
    result
  }
}

///|
pub fn SmallIntSet::intersection(
  self : SmallIntSet,
  other : SmallIntSet,
) -> SmallIntSet {
  let bitmask1 = self.0[0]
  let bitmask2 = other.0[0]
  let mut bitmask = bitmask1 & bitmask2
  let result = Array::make(1 + bitmask.popcnt(), 0)
  let to_remove_data_indices = []
  let mut i = 1
  let mut j = 1
  let mut k = 1
  let mut bitmask1 = bitmask1
  let mut bitmask2 = bitmask2
  while true {
    let ib = bitmask1.ctz()
    let jb = bitmask2.ctz()
    if ib < jb {
      i += 1
      bitmask1 = bitmask1 & (bitmask1 - 1)
    } else if ib > jb {
      j += 1
      bitmask2 = bitmask2 & (bitmask2 - 1)
    } else {
      if ib == 32 {
        break
      }
      result[k] = self.0[i] & other.0[j]
      if result[k] == 0 {
        to_remove_data_indices.push(k)
      }
      i += 1
      j += 1
      k += 1
      bitmask1 = bitmask1 & (bitmask1 - 1)
      bitmask2 = bitmask2 & (bitmask2 - 1)
    }
  }
  for i = to_remove_data_indices.length() - 1; i >= 0; i = i - 1 {
    let k = to_remove_data_indices[i]
    bitmask = clear_nth_set_bit(bitmask, k)
    result.remove(k) |> ignore
  }
  result[0] = bitmask
  result
}

///|
pub fn SmallIntSet::difference(
  self : SmallIntSet,
  other : SmallIntSet,
) -> SmallIntSet {
  let bitmask1 = self.0[0]
  let bitmask2 = other.0[0]
  let mut bitmask = bitmask1
  let result = Array::make(1 + bitmask.popcnt(), 0)
  let to_remove_data_indices = []
  let mut i = 1
  let mut j = 1
  let mut k = 1
  let mut bitmask1 = bitmask1
  let mut bitmask2 = bitmask2
  while true {
    let ib = bitmask1.ctz()
    let jb = bitmask2.ctz()
    if ib < jb {
      result[k] = self.0[i]
      i += 1
      k += 1
      bitmask1 = bitmask1 & (bitmask1 - 1)
    } else if ib > jb {
      j += 1
      bitmask2 = bitmask2 & (bitmask2 - 1)
    } else {
      if ib == 32 {
        break
      }
      result[k] = self.0[i] & other.0[j].lnot()
      if result[k] == 0 {
        to_remove_data_indices.push(k)
      }
      i += 1
      j += 1
      k += 1
      bitmask1 = bitmask1 & (bitmask1 - 1)
      bitmask2 = bitmask2 & (bitmask2 - 1)
    }
  }
  for i = to_remove_data_indices.length() - 1; i >= 0; i = i - 1 {
    let k = to_remove_data_indices[i]
    bitmask = clear_nth_set_bit(bitmask, k)
    result.remove(k) |> ignore
  }
  result[0] = bitmask
  result
}

///|
pub fn SmallIntSet::disjoint(self : SmallIntSet, other : SmallIntSet) -> Bool {
  let bitmask1 = self.0[0]
  let bitmask2 = other.0[0]
  if (bitmask1 & bitmask2) != 0 {
    return false
  }
  let mut i = 1
  let mut j = 1
  let mut bitmask1 = bitmask1
  let mut bitmask2 = bitmask2
  while true {
    let ib = bitmask1.ctz()
    let jb = bitmask2.ctz()
    if ib < jb {
      i += 1
      bitmask1 = bitmask1 & (bitmask1 - 1)
    } else if ib > jb {
      j += 1
      bitmask2 = bitmask2 & (bitmask2 - 1)
    } else {
      if ib == 32 {
        break
      }
      if (self.0[i] & other.0[j]) != 0 {
        return false
      }
      i += 1
      j += 1
      bitmask1 = bitmask1 & (bitmask1 - 1)
      bitmask2 = bitmask2 & (bitmask2 - 1)
    }
  }
  return true
}

///|
pub fn SmallIntSet::subset(self : SmallIntSet, other : SmallIntSet) -> Bool {
  let bitmask1 = self.0[0]
  let bitmask2 = other.0[0]
  if (bitmask1 & bitmask2) != bitmask1 {
    return false
  }
  let mut i = 1
  let mut j = 1
  let mut bitmask1 = bitmask1
  let mut bitmask2 = bitmask2
  while true {
    let ib = bitmask1.ctz()
    let jb = bitmask2.ctz()
    if ib < jb {
      i += 1
      bitmask1 = bitmask1 & (bitmask1 - 1)
    } else if ib > jb {
      j += 1
      bitmask2 = bitmask2 & (bitmask2 - 1)
    } else {
      if ib == 32 {
        break
      }
      if (self.0[i] & other.0[j]) != self.0[i] {
        return false
      }
      i += 1
      j += 1
      bitmask1 = bitmask1 & (bitmask1 - 1)
      bitmask2 = bitmask2 & (bitmask2 - 1)
    }
  }
  return true
}
