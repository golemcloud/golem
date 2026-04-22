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
pub(open) trait CryptoHasher {
  /// Returns the byte-length of hash output
  size(Self) -> Int
  /// Returns the underlying block size of the hasher
  block_size(Self) -> Int
  /// Resets to initial state
  reset(Self) -> Unit
  /// Updates hash state with new data
  update(Self, BytesView) -> Unit
  /// Writes the hash output into the provided buffer.
  /// The buffer must be at least `size(Self)` bytes long after the offset.
  finalize_into(Self, FixedArray[Byte], offset~ : Int) -> Unit
}
