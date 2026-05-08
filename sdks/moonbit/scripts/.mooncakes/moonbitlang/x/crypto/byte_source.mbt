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
trait ByteSource {
  op_get(Self, index : Int) -> Byte
  length(Self) -> Int
  blit_to(
    Self,
    dst : FixedArray[Byte],
    len~ : Int,
    src_offset~ : Int,
    dst_offset~ : Int,
  ) -> Unit
}

///|
pub impl ByteSource for FixedArray[Byte] with op_get(self, index) {
  self[index]
}

///|
pub impl ByteSource for FixedArray[Byte] with length(self) {
  self.length()
}

///|
pub impl ByteSource for FixedArray[Byte] with blit_to(
  self,
  dst,
  len~,
  src_offset~,
  dst_offset~,
) {
  self.blit_to(dst, len~, src_offset~, dst_offset~)
}

///|
pub impl ByteSource for BytesView with op_get(self, index) {
  self[index]
}

///|
pub impl ByteSource for BytesView with length(self) {
  self.length()
}

///|
pub impl ByteSource for BytesView with blit_to(
  self,
  dst,
  len~,
  src_offset~,
  dst_offset~,
) {
  dst.blit_from_bytesview(dst_offset, self[src_offset:src_offset + len])
}

///|
pub impl ByteSource for Bytes with op_get(self, index) {
  self[index]
}

///|
pub impl ByteSource for Bytes with length(self) {
  self.length()
}

///|
pub impl ByteSource for Bytes with blit_to(
  self,
  dst,
  len~,
  src_offset~,
  dst_offset~,
) {
  dst.blit_from_bytes(dst_offset, self, src_offset, len)
}
