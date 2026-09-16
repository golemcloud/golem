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

pub fn value_at_path<'a>(
    root: &'a serde_json::Value,
    path: &[String],
) -> Option<&'a serde_json::Value> {
    let mut current = root;
    for segment in path {
        current = match current {
            serde_json::Value::Object(map) => map.get(segment)?,
            _ => return None,
        };
    }
    Some(current)
}

pub fn collect_leaf_paths(value: &serde_json::Value) -> Vec<Vec<String>> {
    fn collect(value: &serde_json::Value, prefix: &mut Vec<String>, result: &mut Vec<Vec<String>>) {
        match value {
            serde_json::Value::Object(map) => {
                if map.is_empty() {
                    result.push(prefix.clone());
                }
                for (key, nested) in map {
                    prefix.push(key.clone());
                    collect(nested, prefix, result);
                    prefix.pop();
                }
            }
            _ => result.push(prefix.clone()),
        }
    }

    let mut result = Vec::new();
    collect(value, &mut vec![], &mut result);
    result
}

pub fn collect_unused_leaf_paths<F>(
    value: &serde_json::Value,
    mut is_consumed: F,
) -> Vec<Vec<String>>
where
    F: FnMut(&[String]) -> bool,
{
    let mut unused = collect_leaf_paths(value)
        .into_iter()
        .filter(|path| !path.is_empty())
        .filter(|path| !is_consumed(path))
        .collect::<Vec<_>>();

    unused.sort();
    unused.dedup();
    unused
}

#[cfg(test)]
mod test {
    use super::{ProfileView, collect_leaf_paths, collect_unused_leaf_paths, value_at_path};
    use crate::config::{DEFAULT_CLOUD_URL, NamedProfile, Profile, ProfileName};
    use pretty_assertions::assert_eq;
    use serde_json::json;
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

    #[test]
    fn value_at_path_returns_nested_values() {
        let input = json!({
            "a": {
                "b": {
                    "c": 42
                }
            }
        });

        assert_eq!(
            value_at_path(&input, &["a".to_string(), "b".to_string(), "c".to_string()]),
            Some(&json!(42))
        );
    }

    #[test]
    fn value_at_path_returns_none_for_non_objects() {
        let input = json!({ "a": 1 });

        assert_eq!(
            value_at_path(&input, &["a".to_string(), "b".to_string()]),
            None
        );
    }

    #[test]
    fn collect_leaf_paths_collects_all_terminal_paths() {
        let input = json!({
            "a": { "x": 1, "y": true },
            "b": [1,2],
            "c": "v"
        });

        let mut result = collect_leaf_paths(&input)
            .into_iter()
            .map(|path| path.join("."))
            .collect::<Vec<_>>();
        result.sort();

        assert_eq!(result, vec!["a.x", "a.y", "b", "c"]);
    }

    #[test]
    fn collect_leaf_paths_treats_empty_object_as_terminal() {
        let input = json!({
            "db": {},
            "nested": {
                "conn": {}
            }
        });

        let mut result = collect_leaf_paths(&input)
            .into_iter()
            .map(|path| path.join("."))
            .collect::<Vec<_>>();
        result.sort();

        assert_eq!(result, vec!["db", "nested.conn"]);
    }

    #[test]
    fn collect_unused_leaf_paths_supports_exact_match() {
        let input = json!({
            "a": { "x": 1, "y": true },
            "b": [1,2],
            "c": "v"
        });

        let declared = [
            vec!["a".to_string(), "x".to_string()],
            vec!["c".to_string()],
        ];

        let mut result =
            collect_unused_leaf_paths(&input, |path| declared.contains(&path.to_vec()))
                .into_iter()
                .map(|path| path.join("."))
                .collect::<Vec<_>>();
        result.sort();

        assert_eq!(result, vec!["a.y", "b"]);
    }

    #[test]
    fn collect_unused_leaf_paths_supports_prefix_match() {
        let input = json!({
            "db": {
                "user": "u",
                "password": "p"
            },
            "x": 1
        });

        let consumed = [vec!["db".to_string()]];

        let result = collect_unused_leaf_paths(&input, |path| {
            consumed.iter().any(|prefix| path.starts_with(prefix))
        })
        .into_iter()
        .map(|path| path.join("."))
        .collect::<Vec<_>>();

        assert_eq!(result, vec!["x"]);
    }

    #[test]
    fn collect_unused_leaf_paths_filters_root_empty_path() {
        let input = json!({});

        let result = collect_unused_leaf_paths(&input, |_| false);

        assert!(result.is_empty());
    }
}
