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
struct Stamp(Int) derive(Eq, Hash)

///|
pub type T = Stamp

///|
let next_stamp : Ref[Int] = { val: 0 }

///|
let initial_stamp : Stamp = new()

///|
pub fn initial() -> Stamp {
  initial_stamp
}

///|
pub fn new() -> Stamp {
  let stamp = next_stamp.val
  next_stamp.val += 1
  stamp
}

///|
pub fn unsafe_from_int(stamp : Int) -> Stamp {
  stamp
}
