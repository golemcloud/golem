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

pub use crate::base_model::error::*;
use std::fmt::{Debug, Display};
use std::sync::Arc;

#[derive(Clone)]
/// Cloneable erased error
pub struct SharedError(Arc<anyhow::Error>);

impl SharedError {
    pub fn new(e: impl Into<anyhow::Error>) -> Self {
        Self(Arc::new(e.into()))
    }
}

impl Display for SharedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        Display::fmt(&*self.0, f)
    }
}

impl Debug for SharedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Debug::fmt(&*self.0, f)
    }
}

impl std::error::Error for SharedError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.0.source()
    }
    // TODO: this is missing the nightly only trait for proper backtrace propagation
}

impl From<anyhow::Error> for SharedError {
    fn from(e: anyhow::Error) -> Self {
        SharedError::new(e)
    }
}

mod protobuf {

    use crate::model::error::{ErrorBody, ErrorsBody};

    impl From<golem_api_grpc::proto::golem::common::ErrorBody> for ErrorBody {
        fn from(value: golem_api_grpc::proto::golem::common::ErrorBody) -> Self {
            Self {
                error: value.error,
                cause: None,
            }
        }
    }

    impl From<ErrorBody> for golem_api_grpc::proto::golem::common::ErrorBody {
        fn from(value: ErrorBody) -> Self {
            Self { error: value.error }
        }
    }

    impl From<golem_api_grpc::proto::golem::common::ErrorsBody> for ErrorsBody {
        fn from(value: golem_api_grpc::proto::golem::common::ErrorsBody) -> Self {
            Self {
                errors: value.errors,
                cause: None,
            }
        }
    }

    impl From<ErrorsBody> for golem_api_grpc::proto::golem::common::ErrorsBody {
        fn from(value: ErrorsBody) -> Self {
            Self {
                errors: value.errors,
            }
        }
    }
}
