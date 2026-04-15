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
const UPPER = 0x10FFFF

///|
fn random_chars(rand : @random.Rand) -> Char {
  let i = rand.int(limit=UPPER)
  if i >= 0xD800 && i <= 0xDFFF {
    return random_chars(rand)
  }
  Int::unsafe_to_char(i)
}

///|
fn random_bytes(
  rand : @random.Rand,
  n : Int,
  write_char : (@buffer.Buffer, Char) -> Unit,
) -> Bytes {
  let buf = @buffer.new(size_hint=n * 4)
  for _ in 0..<n {
    write_char(buf, random_chars(rand))
  }
  buf.contents()
}

///|
fn random_utf8(rand : @random.Rand, n : Int) -> Bytes {
  random_bytes(rand, n, @encoding.write_utf8_char)
}

///|
fn random_utf16le(rand : @random.Rand, n : Int) -> Bytes {
  random_bytes(rand, n, @encoding.write_utf16le_char)
}

///|
fn random_utf16be(rand : @random.Rand, n : Int) -> Bytes {
  random_bytes(rand, n, @encoding.write_utf16be_char)
}

///|
pub fn bench_decoding_streaming(
  encoding : @encoding.Encoding,
  seed : Bytes,
) -> Unit {
  let random = match encoding {
    UTF8 => random_utf8
    UTF16 | UTF16LE => random_utf16le
    UTF16BE => random_utf16be
  }
  let rand = @random.Rand::chacha8(seed~)
  let test_data = random(rand, 1024000)
  let test_data_group = test_data.to_array().chunks(1000).map(Bytes::from_array)
  let decoder = @encoding.decoder(encoding)
  for td in test_data_group {
    guard (try? decoder.consume(td)) is Ok(_)
  }
  println("decoded data \{test_data.length()}")
}

///|
pub fn bench_decoding_batch(
  encoding : @encoding.Encoding,
  seed : Bytes,
) -> Unit {
  let random = match encoding {
    UTF8 => random_utf8
    UTF16 | UTF16LE => random_utf16le
    UTF16BE => random_utf16be
  }
  let rand = @random.Rand::chacha8(seed~)
  let test_data = random(rand, 1024000)
  let decoder = @encoding.decoder(encoding)
  guard (try? decoder.decode(test_data)) is Ok(_)
  println("decoded data \{test_data.length()}")
}
