// Copyright 2025 International Digital Economy Academy
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

///|
priv struct ParseContext {
  mut offset : Int
  input : String
  end_offset : Int
}

///|
fn ParseContext::make(input : String) -> ParseContext {
  { offset: 0, input, end_offset: input.length() }
}

///|
priv struct CharClass(Array[(Char, Char)])

///|
fn CharClass::of(array : Array[(Char, Char)]) -> CharClass {
  CharClass(array)
}

///|
fn CharClass::contains(self : CharClass, c : Char) -> Bool {
  for left = 0, right = self.0.length(); left < right; {
    let middle = (left + right) / 2
    let (min, max) = self.0[middle]
    if c < min {
      continue left, middle
    } else if c > max {
      continue middle + 1, right
    } else {
      break true
    }
  } else {
    false
  }
}

///|
priv enum Token {
  Null
  True
  False
  Number(Double)
  String(String)
  LBrace
  RBrace
  LBracket
  RBracket
  Comma
  Colon
}
