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

///|
/// Identity of a deployed agent, as returned by the create-agent and
/// invoke-agent endpoints.
pub(all) struct AgentId {
  component_id : String
  agent_id : String
} derive(Debug, Eq)

///|
fn AgentId::from_json(j : Json) -> AgentId raise {
  let m = expect_object(j)
  {
    component_id: expect_string(get_field(m, "componentId")),
    agent_id: expect_string(get_field(m, "agentId")),
  }
}

///|
let rng : @random.Rand = @random.Rand::chacha8(seed=int64_to_bytes(@async.now()))

///|
fn int64_to_bytes(v : Int64) -> Bytes {
  let buf = @buffer.new()
  let mut x = v
  for _ in 0..<8 {
    buf.write_byte((x & 0xFFL).to_byte())
    x = x >> 8
  }
  buf.to_bytes()
}

///|
let hex_digits : String = "0123456789abcdef"

///|
fn byte_to_hex(b : Int) -> String {
  let hi = (b >> 4) & 0xF
  let lo = b & 0xF
  String::from_array([hex_digits.get_char(hi).unwrap(), hex_digits.get_char(lo).unwrap()])
}

///|
/// Generates a random RFC 4122 version 4 UUID string, used for phantom agent
/// identifiers.
pub fn random_uuid() -> String {
  let bytes : Array[Int] = []
  for _ in 0..<16 {
    bytes.push(rng.int(limit=256))
  }
  // Set version (4) and variant (RFC 4122) bits.
  bytes[6] = (bytes[6] & 0x0F) | 0x40
  bytes[8] = (bytes[8] & 0x3F) | 0x80
  let sb = StringBuilder::new()
  for i in 0..<16 {
    if i == 4 || i == 6 || i == 8 || i == 10 {
      sb.write_string("-")
    }
    sb.write_string(byte_to_hex(bytes[i]))
  }
  sb.to_string()
}
