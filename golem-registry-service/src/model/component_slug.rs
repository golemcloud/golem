use golem_common::model::application::ApplicationName;
use golem_common::model::component::ComponentName;
use golem_common::model::environment::EnvironmentName;

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
//
#[derive(Debug, PartialEq, Eq, Hash, Clone)]
pub struct ComponentSlug {
    account_email: Option<String>,
    application_name: Option<String>,
    environment_name: Option<String>,
    component_name: String,
}

impl ComponentSlug {
    pub fn into_parts(
        self,
    ) -> (
        Option<String>,
        Option<ApplicationName>,
        Option<EnvironmentName>,
        ComponentName,
    ) {
        (
            self.account_email,
            self.application_name.map(ApplicationName),
            self.environment_name.map(EnvironmentName),
            ComponentName(self.component_name),
        )
    }

    pub fn parse(str: &str) -> Result<Self, String> {
        // TODO: We probably want more validations here.
        if str.is_empty() {
            Err("Empty component references are not allowed")?;
        };

        let mut parts = str.split("/").collect::<Vec<_>>();

        if parts.is_empty() || parts.len() > 4 {
            Err("Unexpected number of \"/\"-delimited parts in component reference")?
        };

        if parts.iter().any(|p| p.is_empty()) {
            Err("Empty part in the component reference")?
        };

        parts.reverse();

        Ok(ComponentSlug {
            account_email: parts.get(3).map(|s| s.to_string()),
            application_name: parts.get(2).map(|s| s.to_string()),
            environment_name: parts.get(1).map(|s| s.to_string()),
            component_name: parts[0].to_string(), // safe due to the check above
        })
    }
}

#[cfg(test)]
mod tests {
    use super::ComponentSlug;
    use test_r::test;

    #[test]
    fn parse_component() {
        let res = ComponentSlug::parse("foobar");
        assert_eq!(
            res,
            Ok(ComponentSlug {
                account_email: None,
                application_name: None,
                environment_name: None,
                component_name: "foobar".to_string()
            })
        )
    }

    #[test]
    fn parse_environment_component() {
        let res = ComponentSlug::parse("bar/foobar");
        assert_eq!(
            res,
            Ok(ComponentSlug {
                account_email: None,
                application_name: None,
                environment_name: Some("bar".to_string()),
                component_name: "foobar".to_string()
            })
        )
    }

    #[test]
    fn parse_application_environment_component() {
        let res = ComponentSlug::parse("foo/bar/foobar");
        assert_eq!(
            res,
            Ok(ComponentSlug {
                account_email: None,
                application_name: Some("foo".to_string()),
                environment_name: Some("bar".to_string()),
                component_name: "foobar".to_string()
            })
        )
    }

    #[test]
    fn parse_account_application_environment_component() {
        let res = ComponentSlug::parse("foo@golem.cloud/foo/bar/foobar");
        assert_eq!(
            res,
            Ok(ComponentSlug {
                account_email: Some("foo@golem.cloud".to_string()),
                application_name: Some("foo".to_string()),
                environment_name: Some("bar".to_string()),
                component_name: "foobar".to_string()
            })
        )
    }

    #[test]
    fn reject_longer() {
        let res = ComponentSlug::parse("toolong/foo@golem.cloud/foo/bar/foobar");
        assert!(res.is_err())
    }

    #[test]
    fn reject_empty() {
        let res = ComponentSlug::parse("");
        assert!(res.is_err())
    }

    #[test]
    fn reject_empty_group_1() {
        let res = ComponentSlug::parse("foo/");
        assert!(res.is_err())
    }

    #[test]
    fn reject_empty_group_2() {
        let res = ComponentSlug::parse("/foo");
        assert!(res.is_err())
    }
}
