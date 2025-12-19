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

use crate::model::account::AccountId;
use crate::model::validate_lower_kebab_case_identifier;
use crate::{declare_revision, declare_structs, declare_transparent_newtypes, newtype_uuid};
use derive_more::Display;
use std::str::FromStr;

newtype_uuid!(
    ApplicationId,
    golem_api_grpc::proto::golem::common::ApplicationId
);

declare_revision!(ApplicationRevision);

declare_transparent_newtypes! {
    #[derive(Display)]
    pub struct ApplicationName(pub String);
}

impl TryFrom<String> for ApplicationName {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        validate_lower_kebab_case_identifier("Application", &value)?;
        Ok(ApplicationName(value))
    }
}

impl FromStr for ApplicationName {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s.to_string())
    }
}

declare_structs! {
    pub struct Application {
        pub id: ApplicationId,
        pub revision: ApplicationRevision,
        pub account_id: AccountId,
        pub name: ApplicationName,
    }

    pub struct ApplicationCreation {
        pub name: ApplicationName,
    }

    pub struct ApplicationUpdate {
        pub current_revision: ApplicationRevision,
        pub name: Option<ApplicationName>,
    }
}
