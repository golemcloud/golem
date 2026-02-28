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
extern "js" fn ffi_exit(code : Int) -> Unit =
  #|(code) => { process.exit(code); }

///|
pub extern "js" fn println_to_stderr(msg : String) -> Unit =
  #|(msg) => { console.error("%s", msg); }

///|
pub fn[T] exit(code : Int) -> T {
  ffi_exit(code)
  panic()
}
