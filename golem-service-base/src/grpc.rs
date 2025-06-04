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

use golem_common::model::ProjectId;

pub fn proto_project_id_string(
    id: &Option<golem_api_grpc::proto::golem::common::ProjectId>,
) -> Option<String> {
    (*id)
        .and_then(|v| TryInto::<ProjectId>::try_into(v).ok())
        .map(|v| v.to_string())
}
