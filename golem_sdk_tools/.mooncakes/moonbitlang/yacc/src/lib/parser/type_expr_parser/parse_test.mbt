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
test {
  inspect(
    try? parse("Int"),
    content=(
      #|Ok(Constr(pkg=None, "Int", []))
    ),
  )
}

///|
test {
  inspect(
    try? parse("Array[Int]"),
    content=(
      #|Ok(Constr(pkg=None, "Array", [Constr(pkg=None, "Int", [])]))
    ),
  )
}

///|
test {
  inspect(
    try? parse("Array[(Int, Int)]"),
    content=(
      #|Ok(Constr(pkg=None, "Array", [Tuple([Constr(pkg=None, "Int", []), Constr(pkg=None, "Int", [])])]))
    ),
  )
}

///|
test {
  inspect(
    try? parse("Array[(Int, Map[String, (Int, Int) -> Int])]"),
    content=(
      #|Ok(Constr(pkg=None, "Array", [Tuple([Constr(pkg=None, "Int", []), Constr(pkg=None, "Map", [Constr(pkg=None, "String", []), Arrow([Constr(pkg=None, "Int", []), Constr(pkg=None, "Int", [])], Constr(pkg=None, "Int", []))])])]))
    ),
  )
}
