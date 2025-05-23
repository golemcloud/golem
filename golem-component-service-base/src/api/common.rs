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

use golem_api_grpc::proto::golem::component::v1::{component_error, ComponentError};
use golem_common::metrics::api::TraceErrorKind;
use std::fmt::{Debug, Formatter};

pub struct ComponentTraceErrorKind<'a>(pub &'a ComponentError);

impl Debug for ComponentTraceErrorKind<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl TraceErrorKind for ComponentTraceErrorKind<'_> {
    fn trace_error_kind(&self) -> &'static str {
        match &self.0.error {
            None => "None",
            Some(error) => match error {
                component_error::Error::BadRequest(_) => "BadRequest",
                component_error::Error::NotFound(_) => "NotFound",
                component_error::Error::AlreadyExists(_) => "AlreadyExists",
                component_error::Error::LimitExceeded(_) => "LimitExceeded",
                component_error::Error::Unauthorized(_) => "Unauthorized",
                component_error::Error::InternalError(_) => "InternalError",
            },
        }
    }

    fn is_expected(&self) -> bool {
        match &self.0.error {
            None => false,
            Some(error) => match error {
                component_error::Error::BadRequest(_) => true,
                component_error::Error::NotFound(_) => true,
                component_error::Error::AlreadyExists(_) => true,
                component_error::Error::LimitExceeded(_) => true,
                component_error::Error::Unauthorized(_) => true,
                component_error::Error::InternalError(_) => false,
            },
        }
    }
}
