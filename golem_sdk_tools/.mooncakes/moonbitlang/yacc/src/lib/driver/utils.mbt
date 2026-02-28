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
fn offset_to_line_column(content : String, offset : Int) -> (Int, Int) {
  let mut line = 1
  let mut column = 1
  for i in 0..<offset {
    if content.code_unit_at(i) == '\n' {
      line += 1
      column = 1
    } else {
      column += 1
    }
  }
  (line, column)
}

///|
fn array_to_or_list(arr : ArrayView[String]) -> String {
  match arr {
    [] => panic()
    [x] => x
    [.. xs, x] => {
      let left = [..xs].join(", ")
      "\{left} or \{x}"
    }
  }
}

///|
fn loc_to_string(
  filename : String,
  content : String,
  loc : (Int, Int),
) -> String {
  let (start_line, start_column) = offset_to_line_column(content, loc.0)
  let (end_line, end_column) = offset_to_line_column(content, loc.1)
  if start_line == end_line {
    "\{filename}:\{start_line}:\{start_column}-\{end_column}"
  } else {
    "\{filename}:\{start_line}:\{start_column}-\{end_line}:\{end_column}"
  }
}

///|
fn pos_to_string(filename : String, content : String, pos : Int) -> String {
  let (line, column) = offset_to_line_column(content, pos)
  "\{filename}:\{line}:\{column}0"
}
