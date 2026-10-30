// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! This module contains golden tests ensuring that worker related serialized information
//! (such as oplog entries, promises, scheduling, etc.) created by Golem OSS 1.0.0 can be deserialized.
//! Do not regenerate the golden test binaries unless backward compatibility with 1.0 is dropped.
//!
//! The tests are assuming composability of the serializer implementation, so if a given type A has a field of type B,
//! the test for A only contains an example value of B but there exists a separate test that tests the serialization of B.

use desert_rust::BinaryCodec;
use goldenfile::Mint;
use goldenfile::differs::Differ;
use golem_common::serialization::{deserialize, serialize};
use std::fmt::Debug;
use std::io::Write;
use std::path::Path;

#[allow(unused)]
fn is_deserializable<T: BinaryCodec + PartialEq + Debug>(old: &Path, new: &Path) {
    let old = std::fs::read(old).unwrap();
    let new = std::fs::read(new).unwrap();

    // Both the old and the latest binary can be deserialized
    let old_decoded: T = deserialize(&old).unwrap();
    let new_decoded: T = deserialize(&new).unwrap();

    // And they represent the same value
    assert_eq!(old_decoded, new_decoded);
}

#[allow(unused)]
pub(crate) fn backward_compatible_custom<T: BinaryCodec + Debug + 'static>(
    name: impl AsRef<str>,
    mint: &mut Mint,
    value: T,
    differ: Differ,
) {
    let mut file = mint
        .new_goldenfile_with_differ(format!("{}.bin", name.as_ref()), differ)
        .unwrap();
    let encoded = serialize(&value).unwrap();
    file.write_all(&encoded).unwrap();
    file.flush().unwrap();
}

#[allow(unused)]
pub(crate) fn backward_compatible<T: BinaryCodec + PartialEq + Debug + 'static>(
    name: impl AsRef<str>,
    mint: &mut Mint,
    value: T,
) {
    backward_compatible_custom(name, mint, value, Box::new(is_deserializable::<T>))
}
