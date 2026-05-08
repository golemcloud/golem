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

// This file is partially derived from the Python project:
//   https://github.com/python/cpython/blob/3.12/Lib/uuid.py

///|
/// The UUID variant.
pub(all) enum Variant {
  ReservedNCS
  RFC4122(Version)
  ReservedMicrosoft
  ReservedFuture
} derive(Show)

///|
/// The UUID version number.
///
/// This is only used when the variant is `RFC4122`.
///
pub(all) enum Version {
  V1
  V2
  V3
  V4
  V5
  Unknown(Int)
} derive(Show)

///|
fn Version::from_int64(int : Int64) -> Version {
  match int {
    1L => V1
    2L => V2
    3L => V3
    4L => V4
    5L => V5
    ver => Unknown(ver.to_int())
  }
}

///|
fn Version::to_int64(self : Version) -> Int64 {
  match self {
    V1 => 1L
    V2 => 2L
    V3 => 3L
    V4 => 4L
    V5 => 5L
    Unknown(ver) => ver.to_int64()
  }
}

///|
/// Retrieves the variant of the UUID.
pub fn UUID::variant(self : UUID) -> Variant {
  if (self.lo & (0x8000L << 48)) == 0L {
    ReservedNCS
  } else if (self.lo & (0x4000L << 48)) == 0L {
    RFC4122(
      Version::from_int64(
        (self.hi.reinterpret_as_uint64() >> 12).reinterpret_as_int64() & 0xfL,
      ),
    )
  } else if (self.lo & (0x2000L << 48)) == 0L {
    ReservedMicrosoft
  } else {
    ReservedFuture
  }
}

///|
/// Retrieves the version number of the UUID.
///
/// If the variant of the UUID is not `RFC4122`, returns `None`.
///
pub fn UUID::version(self : UUID) -> Version? {
  match self.variant() {
    RFC4122(ver) => Some(ver)
    _ => None
  }
}

///|
/// Converts this UUID to RFC-4122 with the given version number.
pub fn UUID::as_version(self : UUID, version : Version) -> UUID {
  let { hi, lo } = self
  // Set the variant to RFC 4122.
  let lo = (lo & (0xc000L << 48).lnot()) | (0x8000L << 48)
  let hi = (
    // Set the version number.
      hi & 0xf000L.lnot()
    ) |
    (version.to_int64() << 12)
  { hi, lo }
}

///|
test "variant" {
  inspect(
    from_hex("81a277a6-771b-1a55-6246-fa3763226aa0").variant(),
    content="ReservedNCS",
  )
  inspect(
    from_hex("42e0e55d-cee5-044c-b3a6-18a7dc5ae8b4").variant(),
    content="RFC4122(Unknown(0))",
  )
  inspect(
    from_hex("1db64c57-6b48-fd91-dab2-d3accd060480").variant(),
    content="ReservedMicrosoft",
  )
  inspect(
    from_hex("96b2d68d-f97d-9358-f2b1-79cbc6d43c83").variant(),
    content="ReservedFuture",
  )
}

///|
test "version" {
  let u = from_hex("907976bf-6784-5a4f-5d7d-33c04dd8fa1a")
  inspect(u.version(), content="None")
  inspect(u.as_version(V1).version(), content="Some(V1)")
  inspect(u.as_version(V2).version(), content="Some(V2)")
  inspect(u.as_version(V3).version(), content="Some(V3)")
  inspect(u.as_version(V4).version(), content="Some(V4)")
  inspect(u.as_version(V5).version(), content="Some(V5)")
  inspect(u.as_version(Unknown(9)).version(), content="Some(Unknown(9))")
}
