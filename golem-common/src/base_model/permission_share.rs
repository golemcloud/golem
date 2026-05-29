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

use crate::base_model::account::AccountId;
use crate::{declare_revision, declare_structs, declare_transparent_newtypes, newtype_uuid};
use derive_more::Display;
use uuid::Uuid;

newtype_uuid!(PermissionShareId);

declare_revision!(PermissionShareRevision);

declare_transparent_newtypes! {
    #[derive(Display, Eq, Hash, PartialOrd, Ord)]
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", desert(transparent))]
    pub struct PermissionShareName(pub String);
}

declare_structs! {
    pub struct PermissionShareData {
        pub lower_positive: Vec<String>,
        pub lower_negative: Vec<String>,
        pub upper_positive: Vec<String>,
        pub upper_negative: Vec<String>
    }

    pub struct PermissionShare {
        pub id: PermissionShareId,
        pub revision: PermissionShareRevision,
        pub owner_account_id: AccountId,
        pub target_account_id: AccountId,
        pub name: PermissionShareName,
        pub current_card_id: Option<Uuid>,
        pub data: PermissionShareData
    }

    pub struct PermissionShareCreation {
        pub target_account_id: AccountId,
        pub name: PermissionShareName,
        pub data: PermissionShareData
    }

    pub struct PermissionShareUpdate {
        pub current_revision: PermissionShareRevision,
        pub name: PermissionShareName,
        pub data: PermissionShareData
    }
}
