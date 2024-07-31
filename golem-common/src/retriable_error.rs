// Copyright 2024 Golem Cloud
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

use tonic::{Code, Status};

pub trait IsRetriableError {
    fn is_retriable(&self) -> bool;
}

impl IsRetriableError for Status {
    fn is_retriable(&self) -> bool {
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
            | Code::Unauthenticated => false,
            Code::Unknown
            | Code::DeadlineExceeded
            | Code::ResourceExhausted
            | Code::Aborted
            | Code::Internal
            | Code::Unavailable => true,
        }
    }
}
