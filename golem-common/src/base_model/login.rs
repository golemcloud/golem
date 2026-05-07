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

use crate::{declare_enums, declare_structs, declare_unions, newtype_uuid};
use anyhow::anyhow;
use std::str::FromStr;

newtype_uuid!(OAuth2WebflowStateId);

declare_structs! {
    pub struct OAuth2WebflowData {
        pub url: String,
        pub state: OAuth2WebflowStateId,
    }

    /// Starts the browser-based flow.
    /// The callback will redirect to `redirect` with the token secret appended as `token=<secret>`.
    pub struct OAuth2WebflowStartBrowser {
        pub provider: OAuth2Provider,
        pub redirect: String,
    }

    /// Starts the CLI polling flow.
    /// The callback stores the token for retrieval via the poll endpoint.
    /// The browser is redirected to the server's configured CLI redirect URL.
    pub struct OAuth2WebflowStartCli {
        pub provider: OAuth2Provider,
    }
}

declare_unions! {
    pub enum OAuth2WebflowStart {
        Browser(OAuth2WebflowStartBrowser),
        Cli(OAuth2WebflowStartCli),
    }
}

declare_enums! {
    pub enum OAuth2Provider {
        Github,
    }
}

impl FromStr for OAuth2Provider {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "github" => Ok(OAuth2Provider::Github),
            _ => Err(anyhow!("Invalid OAuth2Provider: {s}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow;
    use pretty_assertions::assert_eq;
    use test_r::test;

    #[test]
    fn github_oauth2_provider_roundtrip() -> anyhow::Result<()> {
        assert_eq!(
            OAuth2Provider::from_str(&OAuth2Provider::Github.to_string())?,
            OAuth2Provider::Github
        );
        Ok(())
    }
}
