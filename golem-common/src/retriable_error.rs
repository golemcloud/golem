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
