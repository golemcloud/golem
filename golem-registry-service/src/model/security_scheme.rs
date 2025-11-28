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

use golem_common::model::environment::EnvironmentId;
use golem_common::model::security_scheme::{
    Provider, SecuritySchemeDto, SecuritySchemeId, SecuritySchemeName, SecuritySchemeRevision,
};
use openidconnect::{ClientId, ClientSecret, RedirectUrl, Scope};

#[derive(Debug, Clone)]
pub struct SecurityScheme {
    pub id: SecuritySchemeId,
    pub revision: SecuritySchemeRevision,
    pub name: SecuritySchemeName,
    pub environment_id: EnvironmentId,
    pub provider_type: Provider,
    pub client_id: ClientId,
    pub client_secret: ClientSecret,
    pub redirect_url: RedirectUrl,
    pub scopes: Vec<Scope>,
}

impl PartialEq for SecurityScheme {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
            && self.revision == other.revision
            && self.name == other.name
            && self.environment_id == other.environment_id
            && self.provider_type == other.provider_type
            && self.client_id == other.client_id
            && self.client_secret.secret() == other.client_secret.secret()
            && self.redirect_url == other.redirect_url
            && self.scopes == other.scopes
    }
}

impl From<SecurityScheme> for SecuritySchemeDto {
    fn from(value: SecurityScheme) -> Self {
        Self {
            id: value.id,
            revision: value.revision,
            name: value.name,
            environment_id: value.environment_id,
            provider_type: value.provider_type,
            client_id: value.client_id.into(),
            redirect_url: (*value.redirect_url).clone(),
            scopes: value.scopes.into_iter().map(|s| (*s).clone()).collect(),
        }
    }
}
