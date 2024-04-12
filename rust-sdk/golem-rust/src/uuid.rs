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

use uuid::Uuid;

impl From<crate::bindings::golem::api::host::Uuid> for Uuid {
    fn from(uuid: crate::bindings::golem::api::host::Uuid) -> Self {
        Uuid::from_u64_pair(uuid.high_bits, uuid.low_bits)
    }
}

impl From<Uuid> for crate::bindings::golem::api::host::Uuid {
    fn from(value: Uuid) -> Self {
        let (high_bits, low_bits) = value.as_u64_pair();
        Self {
            high_bits,
            low_bits,
        }
    }
}
