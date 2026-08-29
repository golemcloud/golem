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
/// WebSocket opcode types - internal implementation detail
priv enum OpCode {
  Continuation // 0x0
  Text // 0x1
  Binary // 0x2
  Close // 0x8
  Ping // 0x9
  Pong // 0xA
}

///|
fn OpCode::to_byte(self : OpCode) -> Byte {
  match self {
    Continuation => b'\x00'
    Text => b'\x01'
    Binary => b'\x02'
    Close => b'\x08'
    Ping => b'\x09'
    Pong => b'\x0A'
  }
}

///|
fn OpCode::from_byte(byte : Byte) -> OpCode? {
  match byte {
    b'\x00' => Some(Continuation)
    b'\x01' => Some(Text)
    b'\x02' => Some(Binary)
    b'\x08' => Some(Close)
    b'\x09' => Some(Ping)
    b'\x0A' => Some(Pong)
    _ => None
  }
}

///|
/// WebSocket frame - internal implementation detail
priv struct FrameHeader {
  fin : Bool
  opcode : OpCode
  payload_len : Int64
}

///|
/// WebSocket message
pub(all) enum MessageKind {
  Binary
  Text
} derive(Debug)

///|
/// WebSocket close status codes
pub(all) enum CloseCode {
  Normal // 1000
  GoingAway // 1001
  ProtocolError // 1002
  UnsupportedData // 1003
  Abnormal // 1006
  InvalidFramePayload // 1007
  PolicyViolation // 1008
  MessageTooBig // 1009
  MissingExtension // 1010
  InternalError // 1011
  Other(UInt16)
} derive(Debug, Eq)

///|
fn CloseCode::to_uint16(self : CloseCode) -> UInt16 {
  match self {
    Normal => 1000
    GoingAway => 1001
    ProtocolError => 1002
    UnsupportedData => 1003
    Abnormal => 1006
    InvalidFramePayload => 1007
    PolicyViolation => 1008
    MessageTooBig => 1009
    MissingExtension => 1010
    InternalError => 1011
    Other(i) => i
  }
}

///|
fn CloseCode::from_uint(code : UInt) -> CloseCode {
  match code {
    1000 => Normal
    1001 => GoingAway
    1002 => ProtocolError
    1003 => UnsupportedData
    1006 => Abnormal
    1007 => InvalidFramePayload
    1008 => PolicyViolation
    1009 => MessageTooBig
    1010 => MissingExtension
    1011 => InternalError
    _ => Other(code.to_uint16())
  }
}

///|
pub suberror WebSocketError {
  ConnectionClosed(CloseCode, String?) // Connection was closed
  InvalidHandshake(String) // Handshake failed with specific reason
  HandshakeRejected(String, @http.Response) // Server rejected handshake
  ProtocolError(String)
} derive(Debug)
