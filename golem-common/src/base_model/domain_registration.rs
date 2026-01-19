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

use crate::base_model::environment::EnvironmentId;
use crate::{declare_structs, declare_transparent_newtypes, newtype_uuid};
use derive_more::Display;

newtype_uuid!(DomainRegistrationId);

declare_transparent_newtypes! {
    #[derive(Display, Eq, Hash, PartialOrd, Ord)]
    #[cfg_attr(feature = "full", derive(desert_rust::BinaryCodec))]
    #[cfg_attr(feature = "full", desert(transparent))]
    pub struct Domain(pub String);
}

declare_structs! {
    pub struct DomainRegistrationCreation {
        pub domain: Domain
    }

    pub struct DomainRegistration {
        pub id: DomainRegistrationId,
        pub environment_id: EnvironmentId,
        pub domain: Domain
    }
}
