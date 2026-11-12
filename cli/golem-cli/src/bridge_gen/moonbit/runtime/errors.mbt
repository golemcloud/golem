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
/// `InvocationFailed` is a streaming call that the server reports as a failure
/// of the agent, in a rejection or in the outcome of the finished invocation:
/// `code` is `invocation-failed` or `previous-invocation-failed`, `message` the
/// agent's error text, and `stderr` what the agent wrote to stderr, if anything.
pub(all) suberror BridgeError {
  BridgeError(String)
  Unreachable(endpoint~ : String, window_ms~ : Int32, last_cause~ : String)
  InvocationFailed(code~ : String, message~ : String, stderr~ : String?)
} derive(Debug, Eq)

///|
pub extend BridgeError with Debug::{to_repr}

///|
pub extend BridgeError with Eq::{equal, not_equal}
