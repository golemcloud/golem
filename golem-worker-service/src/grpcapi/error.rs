// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Golem Source License v1.0 (the "License");
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

use golem_api_grpc::proto::golem::worker::v1::WorkerError;
use golem_common::metrics::api::ApiErrorDetails;
use std::fmt::{Debug, Formatter};

pub struct WorkerTraceErrorKind<'a>(pub &'a WorkerError);

impl Debug for WorkerTraceErrorKind<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl ApiErrorDetails for WorkerTraceErrorKind<'_> {
    fn trace_error_kind(&self) -> &'static str {
        use golem_api_grpc::proto::golem::worker::v1::worker_error::Error;

        match &self.0.error {
            None => "None",
            Some(error) => match error {
                Error::BadRequest(_) => "BadRequest",
                Error::Unauthorized(_) => "Unauthorized",
                Error::LimitExceeded(_) => "LimitExceeded",
                Error::NotFound(_) => "NotFound",
                Error::AlreadyExists(_) => "AlreadyExists",
                Error::InternalError(_) => "InternalError",
            },
        }
    }

    fn is_expected(&self) -> bool {
        use golem_api_grpc::proto::golem::worker::v1::worker_error::Error;

        match &self.0.error {
            None => false,
            Some(error) => match error {
                Error::BadRequest(_) => true,
                Error::Unauthorized(_) => true,
                Error::LimitExceeded(_) => true,
                Error::NotFound(_) => true,
                Error::AlreadyExists(_) => true,
                Error::InternalError(_) => false,
            },
        }
    }

    fn take_cause(&mut self) -> Option<anyhow::Error> {
        None
    }
}
