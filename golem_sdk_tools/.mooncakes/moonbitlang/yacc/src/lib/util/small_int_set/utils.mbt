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
fn clear_nth_set_bit(x : Int, n : Int) -> Int {
  let mut count = 0
  let mut temp = x
  while temp != 0 {
    count += 1
    if count == n {
      let rightmost_set_bit = temp & -temp
      return x & rightmost_set_bit.lnot()
    }
    temp = temp & (temp - 1)
  }
  x
}
