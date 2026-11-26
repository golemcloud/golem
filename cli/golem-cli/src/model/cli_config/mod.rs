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

pub mod profile;
pub mod server;

use crate::config::{AuthenticationConfig, ClientConfig, NamedProfile, ProfileConfig, ProfileName};
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ProfileView {
    pub is_active: bool,
    pub name: ProfileName,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub url: Option<Url>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub worker_url: Option<Url>,
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub allow_insecure: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub authenticated: Option<bool>,
    pub config: ProfileConfig,
}

impl ProfileView {
    /// Builds the view of a profile. Built-in profiles show the effective URL of their built-in
    /// server (`builtin_local_url` for `local`, the cloud URL for `cloud`) instead of the
    /// connection fields stored in the config file, which are not used for them.
    pub fn from_profile(
        active: &ProfileName,
        profile: NamedProfile,
        builtin_local_url: &Url,
    ) -> Self {
        let NamedProfile { name, profile } = profile;

        let authenticated = match &profile.auth {
            AuthenticationConfig::OAuth2(inner) => Some(inner.data.is_some()),
            AuthenticationConfig::Static(_) => None,
        };

        let (url, worker_url, allow_insecure) = match name.builtin_server() {
            Some(server) => {
                let client_config = ClientConfig::from_server(&server, builtin_local_url);
                (Some(client_config.registry_url), None, false)
            }
            None => (
                profile.custom_url,
                profile.custom_worker_url,
                profile.allow_insecure,
            ),
        };

        ProfileView {
            is_active: &name == active,
            name,
            url,
            worker_url,
            allow_insecure,
            authenticated,
            config: profile.config,
        }
    }
}

#[cfg(test)]
mod test {
    use super::ProfileView;
    use crate::config::{DEFAULT_CLOUD_URL, NamedProfile, Profile, ProfileName};
    use pretty_assertions::assert_eq;
    use test_r::test;
    use url::Url;

    /// A profile as it may be stored in an older or hand-edited config file.
    fn profile_with_stored_connection() -> Profile {
        Profile {
            custom_url: Some(Url::parse("http://stale-stored-url:1111").unwrap()),
            custom_worker_url: Some(Url::parse("http://stale-stored-worker-url:2222").unwrap()),
            allow_insecure: true,
            ..Profile::default()
        }
    }

    #[test]
    fn builtin_profile_views_show_the_effective_server_not_stored_fields() {
        let builtin_local_url = Url::parse("http://192.0.2.10:9891").unwrap();
        let active = ProfileName::local();

        let local = ProfileView::from_profile(
            &active,
            NamedProfile {
                name: ProfileName::local(),
                profile: profile_with_stored_connection(),
            },
            &builtin_local_url,
        );
        assert_eq!(local.url, Some(builtin_local_url.clone()));
        assert_eq!(local.worker_url, None);
        assert!(!local.allow_insecure);
        assert!(local.is_active);

        let cloud = ProfileView::from_profile(
            &active,
            NamedProfile {
                name: ProfileName::cloud(),
                profile: profile_with_stored_connection(),
            },
            &builtin_local_url,
        );
        assert_eq!(cloud.url, Some(Url::parse(DEFAULT_CLOUD_URL).unwrap()));
        assert_eq!(cloud.worker_url, None);
        assert!(!cloud.allow_insecure);
        assert!(!cloud.is_active);
    }

    #[test]
    fn custom_profile_views_show_stored_fields() {
        let view = ProfileView::from_profile(
            &ProfileName::local(),
            NamedProfile {
                name: ProfileName("my-profile".to_string()),
                profile: profile_with_stored_connection(),
            },
            &Url::parse("http://localhost:9881").unwrap(),
        );
        assert_eq!(
            view.url,
            Some(Url::parse("http://stale-stored-url:1111").unwrap())
        );
        assert_eq!(
            view.worker_url,
            Some(Url::parse("http://stale-stored-worker-url:2222").unwrap())
        );
        assert!(view.allow_insecure);
    }
}
