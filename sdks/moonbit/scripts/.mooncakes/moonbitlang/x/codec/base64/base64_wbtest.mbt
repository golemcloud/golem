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
struct CustomBytes(BytesView)

///|
impl Show for CustomBytes with output(self, logger) {
  for b in self.0 {
    if b is (' '..='~') {
      logger.write_char(b.to_char())
    } else {
      logger.write_string(
        "\\x\{b.to_uint().to_string(radix=16).pad_start(2, '0')}",
      )
    }
  }
}

///|
test "base64 encode" {
  inspect(encode(b"light w"), content="bGlnaHQgdw==")
  inspect(encode(b"light wo"), content="bGlnaHQgd28=")
  inspect(encode(b"light wor"), content="bGlnaHQgd29y")
  inspect(encode(b"light work"), content="bGlnaHQgd29yaw==")
  inspect(encode(b"light work."), content="bGlnaHQgd29yay4=")
  inspect(encode(b"a Ā 𐀀 文 🦄"), content="YSDEgCDwkICAIOaWhyDwn6aE")
}

///|
test "base64 decode" {
  inspect(CustomBytes(decode("bGlnaHQgdw==")), content="light w")
  inspect(CustomBytes(decode("bGlnaHQgd28=")), content="light wo")
  inspect(CustomBytes(decode("bGlnaHQgd29y")), content="light wor")
  inspect(CustomBytes(decode("bGlnaHQgd29yaw==")), content="light work")
  inspect(CustomBytes(decode("bGlnaHQgd29yay4=")), content="light work.")
  inspect(
    CustomBytes(decode("YSDEgCDwkICAIOaWhyDwn6aE")),
    content="a \\xc4\\x80 \\xf0\\x90\\x80\\x80 \\xe6\\x96\\x87 \\xf0\\x9f\\xa6\\x84",
  )
}

///|
test "base64 decode with errors" {
  inspect(try? decode("中文"), content="Err(InvalidChar('中'))")
}
