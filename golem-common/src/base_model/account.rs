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

use crate::base_model::auth::AccountRole;
use crate::base_model::plan::PlanId;
use crate::{declare_revision, declare_structs, declare_transparent_newtypes, newtype_uuid};
use derive_more::Display;
use uuid::uuid;

newtype_uuid!(AccountId, wit_name: "account-id", wit_owner: "golem:core@1.5.0/types", golem_api_grpc::proto::golem::common::AccountId);

impl AccountId {
    pub const SYSTEM: Self = AccountId(uuid!("00000000-0000-0000-0000-000000000000"));
}

declare_revision!(AccountRevision);

declare_transparent_newtypes! {
    #[derive(Display)]
    pub struct AccountEmail(pub String);
}

declare_structs! {
    pub struct Account {
        pub id: AccountId,
        pub revision: AccountRevision,
        pub name: String,
        pub email: AccountEmail,
        pub plan_id: PlanId,
        pub roles: Vec<AccountRole>
    }

    pub struct AccountSummary {
        pub id: AccountId,
        pub name: String,
        pub email: AccountEmail,
    }

    pub struct AccountCreation {
        pub name: String,
        pub email: AccountEmail,
    }

    pub struct AccountUpdate {
        pub current_revision: AccountRevision,
        pub name: Option<String>,
        pub email: Option<AccountEmail>,
    }

    pub struct AccountSetRoles {
        pub current_revision: AccountRevision,
        pub roles: Vec<AccountRole>
    }

    pub struct AccountSetPlan {
        pub current_revision: AccountRevision,
        pub plan: PlanId
    }
}
