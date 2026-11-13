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
/// Error raised by the generated bridge client and its runtime on a wire-shape
/// mismatch, a failed HTTP request, or a protocol decoding failure.
/// `Unreachable` is a streaming call that could not reach the server within
/// its reconnect window: `endpoint` is the WebSocket URL, `window_ms` the
/// window, and `last_cause` the last failure, such as the HTTP status of a
/// refused upgrade, a retryable rejection, or a connection error.
/// `StreamFailed` retains the producer's typed failure. `ConsumerCancelled`
/// carries the optional diagnostic from a consumer terminal.
pub(all) suberror BridgeError {
  BridgeError(String)
  Unreachable(endpoint~ : String, window_ms~ : Int32, last_cause~ : String)
  StreamFailed(StreamFailure)
  ConsumerCancelled(String?)
} derive(Debug, Eq)

///|
/// The producer's terminal failure. Failed retains the complete typed value,
/// including its schema graph, so callers can inspect application errors.
pub(all) enum StreamFailure {
  Failed(graph~ : Json, value~ : Json)
  Trapped(String)
  Cancelled
  Abandoned
  Interrupted(String)
  ResourceExhausted(String)
} derive(Debug, Eq)

///|
pub impl Show for BridgeError with fn output(self, logger) {
  match self {
    BridgeError(message) => logger.write_string(message)
    Unreachable(endpoint~, window_ms~, last_cause~) =>
      logger.write_string(
        "Unreachable \{endpoint} after \{window_ms} ms: \{last_cause}",
      )
    StreamFailed(failure) =>
      logger.write_string("Stream failed: \{@debug.to_string(failure)}")
    ConsumerCancelled(details) => {
      logger.write_string("Stream consumer cancelled")
      match details {
        Some(details) => logger.write_string(": \{details}")
        None => ()
      }
    }
  }
}

///|
pub extend BridgeError with Debug::{to_repr}

///|
pub extend BridgeError with Eq::{equal, not_equal}

///|
pub extend BridgeError with Show::{to_string, output}

///|
pub extend StreamFailure with Debug::{to_repr}

///|
pub extend StreamFailure with Eq::{equal, not_equal}
