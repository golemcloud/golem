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
#alias(T)
struct Int128IntSet {
  a : Int
  b : Int
  c : Int
  d : Int
} derive(Eq, Compare, Hash)

///|
pub fn empty() -> Int128IntSet {
  { a: 0, b: 0, c: 0, d: 0 }
}

///|
pub fn Int128IntSet::is_singleton(self : Int128IntSet, key : Int) -> Bool {
  if key < 64 {
    if key < 32 {
      self.a == 1 << key && self.b == 0 && self.c == 0 && self.d == 0
    } else {
      self.a == 0 && self.b == 1 << (key - 32) && self.c == 0 && self.d == 0
    }
  } else if key < 96 {
    self.a == 0 && self.b == 0 && self.c == 1 << (key - 64) && self.d == 0
  } else {
    self.a == 0 && self.b == 0 && self.c == 0 && self.d == 1 << (key - 96)
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
//       match yield_(bit + 32) {
//         IterEnd => return IterEnd
//         IterContinue => ()
//       }
//       bitmask = bitmask & (bitmask - 1)
//     }
//     let mut bitmask = self.c
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
//     let mut bitmask = self.d
//     while true {
//       if bitmask == 0 {
//         break
//       }
//       let bit = bitmask.ctz()
//       match yield_(bit + 96) {
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
  let mut offset = 0
  let int128intset_rest = [(self.d, 96), (self.c, 64), (self.b, 32)]
  Iter::new(fn() {
    if bitmask == 0 {
      if int128intset_rest.pop() is Some((new_bitmask, new_offset)) {
        bitmask = new_bitmask
        offset = new_offset
      } else {
        return None
      }
    }
    let bit = bitmask.ctz()
    bitmask = bitmask & (bitmask - 1)
    return Some(bit + offset)
  })
}

///|
pub fn from_iter(iter : Iter[Int]) -> Int128IntSet {
  let mut a = 0
  let mut b = 0
  let mut c = 0
  let mut d = 0
  for key in iter {
    if key < 64 {
      if key < 32 {
        a = a | (1 << key)
      } else {
        b = b | (1 << (key - 32))
      }
    } else if key < 96 {
      c = c | (1 << (key - 64))
    } else {
      d = d | (1 << (key - 96))
    }
  }
  { a, b, c, d }
}

///|
pub fn singleton(key : Int) -> Int128IntSet {
  if key < 64 {
    if key < 32 {
      { a: 1 << key, b: 0, c: 0, d: 0 }
    } else {
      { a: 0, b: 1 << (key - 32), c: 0, d: 0 }
    }
  } else if key < 96 {
    { a: 0, b: 0, c: 1 << (key - 64), d: 0 }
  } else {
    { a: 0, b: 0, c: 0, d: 1 << (key - 96) }
  }
}

///|
pub fn Int128IntSet::is_empty(self : Int128IntSet) -> Bool {
  self.a == 0 && self.b == 0 && self.c == 0 && self.d == 0
}

///|
pub fn Int128IntSet::contains(self : Int128IntSet, key : Int) -> Bool {
  if key < 64 {
    if key < 32 {
      (self.a & (1 << key)) != 0
    } else {
      (self.b & (1 << (key - 32))) != 0
    }
  } else if key < 96 {
    (self.c & (1 << (key - 64))) != 0
  } else {
    (self.d & (1 << (key - 96))) != 0
  }
}

///|
pub fn Int128IntSet::add(self : Int128IntSet, key : Int) -> Int128IntSet {
  if key < 64 {
    if key < 32 {
      { a: self.a | (1 << key), b: self.b, c: self.c, d: self.d }
    } else {
      { a: self.a, b: self.b | (1 << (key - 32)), c: self.c, d: self.d }
    }
  } else if key < 96 {
    { a: self.a, b: self.b, c: self.c | (1 << (key - 64)), d: self.d }
  } else {
    { a: self.a, b: self.b, c: self.c, d: self.d | (1 << (key - 96)) }
  }
}

///|
pub fn Int128IntSet::remove(self : Int128IntSet, key : Int) -> Int128IntSet {
  if key < 64 {
    if key < 32 {
      { a: self.a & (1 << key).lnot(), b: self.b, c: self.c, d: self.d }
    } else {
      { a: self.a, b: self.b & (1 << (key - 32)).lnot(), c: self.c, d: self.d }
    }
  } else if key < 96 {
    { a: self.a, b: self.b, c: self.c & (1 << (key - 64)).lnot(), d: self.d }
  } else {
    { a: self.a, b: self.b, c: self.c, d: self.d & (1 << (key - 96)).lnot() }
  }
}

///|
pub fn Int128IntSet::union(
  self : Int128IntSet,
  other : Int128IntSet,
) -> Int128IntSet {
  {
    a: self.a | other.a,
    b: self.b | other.b,
    c: self.c | other.c,
    d: self.d | other.d,
  }
}

///|
pub fn Int128IntSet::intersection(
  self : Int128IntSet,
  other : Int128IntSet,
) -> Int128IntSet {
  {
    a: self.a & other.a,
    b: self.b & other.b,
    c: self.c & other.c,
    d: self.d & other.d,
  }
}

///|
pub fn Int128IntSet::difference(
  self : Int128IntSet,
  other : Int128IntSet,
) -> Int128IntSet {
  {
    a: self.a & other.a.lnot(),
    b: self.b & other.b.lnot(),
    c: self.c & other.c.lnot(),
    d: self.d & other.d.lnot(),
  }
}

///|
pub fn Int128IntSet::disjoint(
  self : Int128IntSet,
  other : Int128IntSet,
) -> Bool {
  (self.a & other.a) == 0 &&
  (self.b & other.b) == 0 &&
  (self.c & other.c) == 0 &&
  (self.d & other.d) == 0
}

///|
pub fn Int128IntSet::subset(self : Int128IntSet, other : Int128IntSet) -> Bool {
  (self.a & other.a) == self.a &&
  (self.b & other.b) == self.b &&
  (self.c & other.c) == self.c &&
  (self.d & other.d) == self.d
}
