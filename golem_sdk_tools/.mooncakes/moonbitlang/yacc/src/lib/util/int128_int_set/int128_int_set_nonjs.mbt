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
struct Int128IntSet {
  a : Int64
  b : Int64
} derive(Eq, Compare, Hash)

///|
pub type T = Int128IntSet

///|
pub fn empty() -> Int128IntSet {
  { a: 0, b: 0 }
}

///|
pub fn Int128IntSet::is_singleton(self : Int128IntSet, key : Int) -> Bool {
  if key < 64 {
    self.a == 1L << key && self.b == 0
  } else {
    self.a == 0 && self.b == 1L << (key - 64)
  }
}

///|
pub impl Show for Int128IntSet with output(self, logger) {
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
// pub fn Int128IntSet::iter(self : Int128IntSet) -> Iter[Int] {
//   Iter::new(fn(yield_) {
//     let mut bitmask = self.a
//     while true {
//       if bitmask == 0 {
//         break
//       }
//       let bit = bitmask.ctz()
//       match yield_(bit) {
//         IterEnd => return IterEnd
//         IterContinue => ()
//       }
//       bitmask = bitmask & (bitmask - 1)
//     }
//     let mut bitmask = self.b
//     while true {
//       if bitmask == 0 {
//         break
//       }
//       let bit = bitmask.ctz()
//       match yield_(bit + 64) {
//         IterEnd => return IterEnd
//         IterContinue => ()
//       }
//       bitmask = bitmask & (bitmask - 1)
//     }
//     IterContinue
//   })
// }

pub fn Int128IntSet::iter(self : Int128IntSet) -> Iter[Int] {
  let mut bitmask = self.a
  let mut has_rest = true
  let mut offset = 0
  Iter::new(fn() {
    if bitmask == 0 {
      if has_rest {
        bitmask = self.b
        has_rest = false
        offset = 64
        if bitmask == 0 {
          return None
        }
      } else {
        return None
      }
    }
    let bit = bitmask.clz()
    bitmask = bitmask & (bitmask - 1)
    return Some(bit + offset)
  })
}

///|
pub fn from_iter(iter : Iter[Int]) -> Int128IntSet {
  let mut a = 0L
  let mut b = 0L
  for key in iter {
    if key < 64 {
      a = a | (1L << key)
    } else {
      b = b | (1L << (key - 64))
    }
  }
  { a, b }
}

///|
pub fn singleton(key : Int) -> Int128IntSet {
  if key < 64 {
    { a: 1L << key, b: 0 }
  } else {
    { a: 0, b: 1L << (key - 64) }
  }
}

///|
pub fn Int128IntSet::is_empty(self : Int128IntSet) -> Bool {
  self.a == 0 && self.b == 0
}

///|
pub fn Int128IntSet::contains(self : Int128IntSet, key : Int) -> Bool {
  if key < 64 {
    (self.a & (1L << key)) != 0
  } else {
    (self.b & (1L << (key - 64))) != 0
  }
}

///|
pub fn Int128IntSet::add(self : Int128IntSet, key : Int) -> Int128IntSet {
  if key < 64 {
    { a: self.a | (1L << key), b: self.b }
  } else {
    { a: self.a, b: self.b | (1L << (key - 64)) }
  }
}

///|
pub fn Int128IntSet::remove(self : Int128IntSet, key : Int) -> Int128IntSet {
  if key < 64 {
    { a: self.a & (1L << key).lnot(), b: self.b }
  } else {
    { a: self.a, b: self.b & (1L << (key - 64)).lnot() }
  }
}

///|
pub fn Int128IntSet::union(
  self : Int128IntSet,
  other : Int128IntSet,
) -> Int128IntSet {
  { a: self.a | other.a, b: self.b | other.b }
}

///|
pub fn Int128IntSet::intersection(
  self : Int128IntSet,
  other : Int128IntSet,
) -> Int128IntSet {
  { a: self.a & other.a, b: self.b & other.b }
}

///|
pub fn Int128IntSet::difference(
  self : Int128IntSet,
  other : Int128IntSet,
) -> Int128IntSet {
  { a: self.a & other.a.lnot(), b: self.b & other.b.lnot() }
}

///|
pub fn Int128IntSet::disjoint(
  self : Int128IntSet,
  other : Int128IntSet,
) -> Bool {
  (self.a & other.a) == 0 && (self.b & other.b) == 0
}

///|
pub fn Int128IntSet::subset(self : Int128IntSet, other : Int128IntSet) -> Bool {
  (self.a & other.a) == self.a && (self.b & other.b) == self.b
}
