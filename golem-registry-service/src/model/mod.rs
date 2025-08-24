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

pub mod auth;
pub mod component;
pub mod environment;
pub mod login;

use golem_common::model::account::AccountId;
use golem_common::model::auth::EnvironmentRole;
use std::collections::HashSet;

#[derive(Debug)]
pub struct WithEnvironmentAuth<A> {
    pub value: A,

    pub owner_account_id: AccountId,
    pub roles_from_shares: HashSet<EnvironmentRole>,
}
