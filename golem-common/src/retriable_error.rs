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

pub trait IsRetriableError {
    /// Returns true if the error is retriable.
    fn is_retriable(&self) -> bool;

    /// Returns a loggable string representation of the error. If it returns None, the error
    /// will not be logged and traced.
    ///
    /// This is useful to accept some errors as the expected response (such as a not-found result
    /// when looking for a resource).
    fn as_loggable(&self) -> Option<String>;
}

impl IsRetriableError for tonic::Status {
    fn is_retriable(&self) -> bool {
        use tonic::Code;

        match self.code() {
            Code::Ok
            | Code::Cancelled
            | Code::InvalidArgument
            | Code::NotFound
            | Code::AlreadyExists
            | Code::PermissionDenied
            | Code::FailedPrecondition
            | Code::OutOfRange
            | Code::Unimplemented
            | Code::DataLoss
            | Code::Unauthenticated
            // ResourceExhausted is treated as non-retriable. Within this codebase it covers two
            // categories of failures, neither of which should be retried automatically:
            //  1. Domain-level quota/limit-exceeded errors (see `error_to_status` mapping
            //     `agent_error::LimitExceeded` to `Status::resource_exhausted`).
            //  2. tonic-internal codec/compression size-limit errors (e.g. encoded or decoded
            //     message exceeds `max_(en|de)coding_message_size`). Retrying these will keep
            //     producing the same error and previously caused infinite retry loops.
            | Code::ResourceExhausted => false,
            Code::Unknown
            | Code::DeadlineExceeded
            | Code::Aborted
            | Code::Internal
            | Code::Unavailable => true,
        }
    }

    fn as_loggable(&self) -> Option<String> {
        Some(self.to_string())
    }
}

/// Returns true if the given gRPC status was produced by tonic's codec because the
/// encoded or decoded message exceeded the configured `max_(en|de)coding_message_size`.
///
/// Tonic surfaces these as `Code::ResourceExhausted` with messages such as
/// `"Error decompressing: size limit, of N bytes, exceeded while decompressing message"`
/// or `"Error encoding: size limit, of N bytes, exceeded while encoding message"`.
pub fn is_grpc_message_size_limit(status: &tonic::Status) -> bool {
    if status.code() != tonic::Code::ResourceExhausted {
        return false;
    }
    let message = status.message();
    message.contains("size limit") && message.contains("message")
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_r::test;
    use tonic::{Code, Status};

    #[test]
    fn resource_exhausted_is_not_retriable() {
        let status = Status::new(
            Code::ResourceExhausted,
            "Error decompressing: size limit, of 4194304 bytes, exceeded while decompressing message",
        );
        assert!(!status.is_retriable());
    }

    #[test]
    fn unavailable_is_retriable() {
        let status = Status::new(Code::Unavailable, "service unavailable");
        assert!(status.is_retriable());
    }

    #[test]
    fn not_found_is_not_retriable() {
        let status = Status::new(Code::NotFound, "not found");
        assert!(!status.is_retriable());
    }

    #[test]
    fn message_size_limit_is_detected() {
        let status = Status::new(
            Code::ResourceExhausted,
            "Error decompressing: size limit, of 4194304 bytes, exceeded while decompressing message",
        );
        assert!(is_grpc_message_size_limit(&status));
    }

    #[test]
    fn limit_exceeded_is_not_message_size_limit() {
        let status = Status::new(Code::ResourceExhausted, "quota exceeded");
        assert!(!is_grpc_message_size_limit(&status));
    }

    #[test]
    fn non_resource_exhausted_is_not_message_size_limit() {
        let status = Status::new(Code::Unavailable, "size limit message");
        assert!(!is_grpc_message_size_limit(&status));
    }
}
