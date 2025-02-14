// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

/// User provided reference to a component that can be resolved to a component id.
/// Three forms are accepted:
/// 1: account_id/project_name/component_name
/// 2: project_name/component_name (uses current account)
/// 3: component_name (uses current account and project)
#[derive(Debug, PartialEq, Eq)]
pub struct ComponentSlug {
    account_id: Option<String>,
    project_name: Option<String>,
    component_name: String
}

impl ComponentSlug {

    pub fn parse(str: &str) -> Result<Self, String> {
        // TODO: We probably want more validations here.
        if str.is_empty() {
            Err("Empty component references are not allowed")?;
        };

        if str.contains(" ") {
            Err("No spaces allowed in component reference")?;
        };

        let mut parts = str.split("/").collect::<Vec<_>>();

        if parts.is_empty() || parts.len() > 3  {
            Err("Unexpected number of \"/\"-delimited parts in component reference")?
        };

        parts.reverse();

        Ok(ComponentSlug {
            account_id: parts.get(2).map(|s| s.to_string()),
            project_name: parts.get(1).map(|s| s.to_string()),
            component_name: parts[0].to_string() // safe due to the check above
        })
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;
    use super::ComponentSlug;

    #[test]
    fn parse_component() {
         let res = ComponentSlug::parse("foobar");
         assert_eq!(res, Ok(ComponentSlug {
            account_id: None,
            project_name: None,
            component_name: "foobar".to_string()
         }))
    }

    #[test]
    fn parse_project_component() {
         let res = ComponentSlug::parse("bar/foobar");
         assert_eq!(res, Ok(ComponentSlug {
            account_id: None,
            project_name: Some("bar".to_string()),
            component_name: "foobar".to_string()
         }))
    }


    #[test]
    fn parse_account_project_component() {
         let res = ComponentSlug::parse("baz/bar/foobar");
         assert_eq!(res, Ok(ComponentSlug {
            account_id: Some("baz".to_string()),
            project_name: Some("bar".to_string()),
            component_name: "foobar".to_string()
         }))
    }

    #[test]
    fn reject_longer() {
         let res = ComponentSlug::parse("foo/baz/bar/foobar");
         assert!(matches!(res, Err(_)))
    }

    #[test]
    fn reject_empty() {
         let res = ComponentSlug::parse("");
         assert!(matches!(res, Err(_)))
    }

    #[test]
    fn reject_spaces() {
         let res = ComponentSlug::parse("baz/bar baz/foobar");
         assert!(matches!(res, Err(_)))
    }

}
