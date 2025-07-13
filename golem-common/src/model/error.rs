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

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
pub struct ErrorsBody {
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "poem", derive(poem_openapi::Object))]
#[cfg_attr(feature = "poem", oai(rename_all = "camelCase"))]
pub struct ErrorBody {
    pub error: String,
}

#[cfg(feature = "protobuf")]
mod protobuf {

    use crate::model::error::{ErrorBody, ErrorsBody};

    impl From<golem_api_grpc::proto::golem::common::ErrorBody> for ErrorBody {
        fn from(value: golem_api_grpc::proto::golem::common::ErrorBody) -> Self {
            Self { error: value.error }
        }
    }

    impl From<golem_api_grpc::proto::golem::common::ErrorsBody> for ErrorsBody {
        fn from(value: golem_api_grpc::proto::golem::common::ErrorsBody) -> Self {
            Self {
                errors: value.errors,
            }
        }
    }
}
