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

use golem_common::SafeDisplay;
use http::StatusCode;
use poem::Body;

pub trait ToHttpResponseFromSafeDisplay {
    fn to_response_from_safe_display<F>(&self, get_status_code: F) -> poem::Response
    where
        Self: SafeDisplay,
        F: Fn(&Self) -> StatusCode,
        Self: Sized;
}

// Only SafeDisplay'd errors are allowed to be embedded in any output response
impl<E: SafeDisplay> ToHttpResponseFromSafeDisplay for E {
    fn to_response_from_safe_display<F>(&self, get_status_code: F) -> poem::Response
    where
        F: Fn(&Self) -> StatusCode,
        Self: Sized,
    {
        poem::Response::builder()
            .status(get_status_code(self))
            .body(Body::from_string(self.to_safe_string()))
    }
}
