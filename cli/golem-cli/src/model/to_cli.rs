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

use crate::model::to_oss::ToOss;

pub trait ToCli<T> {
    fn to_cli(self) -> T;
}

impl<A: ToCli<B>, B> ToCli<Option<B>> for Option<A> {
    fn to_cli(self) -> Option<B> {
        self.map(|v| v.to_cli())
    }
}

impl<A: ToCli<B>, B> ToCli<Vec<B>> for Vec<A> {
    fn to_cli(self) -> Vec<B> {
        self.into_iter().map(|v| v.to_cli()).collect()
    }
}

impl ToCli<crate::model::ApiDeployment> for golem_cloud_client::model::ApiDeployment {
    fn to_cli(self) -> crate::model::ApiDeployment {
        crate::model::ApiDeployment {
            api_definitions: self.api_definitions.to_oss(),
            project_id: Some(self.project_id),
            site: self.site.to_oss(),
            created_at: self.created_at,
        }
    }
}

impl ToCli<crate::model::ApiSecurityScheme> for golem_cloud_client::model::SecuritySchemeData {
    fn to_cli(self) -> crate::model::ApiSecurityScheme {
        crate::model::ApiSecurityScheme {
            scheme_identifier: self.scheme_identifier,
            client_id: self.client_id,
            client_secret: self.client_secret,
            redirect_url: self.redirect_url,
            scopes: self.scopes,
        }
    }
}
