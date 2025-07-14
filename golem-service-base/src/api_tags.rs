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

use poem_openapi::Tags;

#[derive(Tags)]
pub enum ApiTags {
    /// The account API allows users to query and manipulate their own account data.
    Account,
    AccountSummary,
    ApiCertificate,
    ApiDefinition,
    ApiDeployment,
    ApiDomain,
    ApiSecurity,
    Component,
    Debugging,
    Grant,
    HealthCheck,
    /// The limits API allows users to query their current resource limits.
    Limits,
    /// The login endpoints are implementing an OAuth2 flow.
    Login,
    Plugin,
    /// Projects are groups of components and their workers, providing both a separate namespace for these entities and allows sharing between accounts.
    ///
    /// Every account has a default project which is assumed when no specific project ID is passed in some component and worker related APIs.
    Project,
    /// Projects can have grants providing access to other accounts than the project's owner.
    ///
    /// The project grant API allows listing, creating and deleting such grants. What the grants allow exactly are defined by policies, covered by the Project policy API.
    ProjectGrant,
    /// Project policies describe a set of actions one account can perform when it was associated with a grant for a project.
    ProjectPolicy,
    /// The token API allows creating custom access tokens for the Golem Cloud REST API to be used by tools and services.
    Token,
    Worker,
}
