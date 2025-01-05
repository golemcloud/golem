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

use crate::model::ComponentVersion;
use crate::uri::{
    try_from_golem_url, urldecode, GolemUrl, GolemUrlTransformError, TypedGolemUrl,
    API_DEFINITION_TYPE_NAME, API_DEPLOYMENT_TYPE_NAME, COMPONENT_TYPE_NAME, WORKER_TYPE_NAME,
};
use crate::url_from_into;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt::{Display, Formatter};
use std::str::FromStr;

/// Typed Golem URL for component
///
/// Format: `component:///{name}`
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ComponentUrl {
    pub name: String,
}

const CLOUD_CONTEXT_ACCOUNT: &str = "account";
const CLOUD_CONTEXT_PROJECT: &str = "project";
const CLOUD_CONTEXT: &[&str] = &[CLOUD_CONTEXT_ACCOUNT, CLOUD_CONTEXT_PROJECT];

impl TypedGolemUrl for ComponentUrl {
    fn resource_type() -> &'static str {
        COMPONENT_TYPE_NAME
    }

    fn try_from_parts(path: &str, query: Option<&str>) -> Result<Self, GolemUrlTransformError>
    where
        Self: Sized,
    {
        let name = Self::expect_path1(path)?;
        Self::expect_empty_query(query, CLOUD_CONTEXT)?;

        Ok(Self { name })
    }

    fn to_parts(&self) -> (String, Option<String>) {
        (Self::make_path1(&self.name), None)
    }
}

url_from_into!(ComponentUrl);

/// Typed Golem URL for component version
///
/// Format: `component:///{name}/{version}`
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ComponentVersionUrl {
    pub name: String,
    pub version: ComponentVersion,
}

impl TypedGolemUrl for ComponentVersionUrl {
    fn resource_type() -> &'static str {
        COMPONENT_TYPE_NAME
    }

    fn try_from_parts(path: &str, query: Option<&str>) -> Result<Self, GolemUrlTransformError>
    where
        Self: Sized,
    {
        let (name, version) = Self::expect_path2(path)?;
        let version: ComponentVersion = version
            .parse()
            .map_err(|err| Self::invalid_path(format!("Failed to parse version: {err}")))?;

        Self::expect_empty_query(query, CLOUD_CONTEXT)?;

        Ok(Self { name, version })
    }

    fn to_parts(&self) -> (String, Option<String>) {
        (
            Self::make_path2(&self.name, &self.version.to_string()),
            None,
        )
    }
}

url_from_into!(ComponentVersionUrl);

/// Typed Golem URL for component or component version
///
/// Format: `component:///{name}` or `component:///{name}/{version}`
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ComponentOrVersionUrl {
    Component(ComponentUrl),
    Version(ComponentVersionUrl),
}

impl TypedGolemUrl for ComponentOrVersionUrl {
    fn resource_type() -> &'static str {
        COMPONENT_TYPE_NAME
    }

    fn try_from_parts(path: &str, query: Option<&str>) -> Result<Self, GolemUrlTransformError>
    where
        Self: Sized,
    {
        if path.strip_prefix('/').unwrap_or("").contains('/') {
            Ok(ComponentOrVersionUrl::Version(
                ComponentVersionUrl::try_from_parts(path, query)?,
            ))
        } else {
            Ok(ComponentOrVersionUrl::Component(
                ComponentUrl::try_from_parts(path, query)?,
            ))
        }
    }

    fn to_parts(&self) -> (String, Option<String>) {
        match self {
            ComponentOrVersionUrl::Component(c) => c.to_parts(),
            ComponentOrVersionUrl::Version(v) => v.to_parts(),
        }
    }
}

url_from_into!(ComponentOrVersionUrl);

/// Typed Golem URL for worker
///
/// Format: `worker:///{component_name}/{worker_name}`
/// or `worker:///{component_name}` for targeting a new ephemeral worker
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct WorkerUrl {
    pub component_name: String,
    pub worker_name: Option<String>,
}

impl TypedGolemUrl for WorkerUrl {
    fn resource_type() -> &'static str {
        WORKER_TYPE_NAME
    }

    fn try_from_parts(path: &str, query: Option<&str>) -> Result<Self, GolemUrlTransformError>
    where
        Self: Sized,
    {
        let path = path
            .strip_prefix('/')
            .ok_or(Self::invalid_path("path is not started with '/'"))?;
        let segments = path.split('/').collect::<Vec<_>>();

        if segments.len() != 1 && segments.len() != 2 {
            Err(Self::invalid_path(format!(
                "1 or 2 segments expected, but got {} segments",
                segments.len()
            )))
        } else {
            Self::expect_empty_query(query, CLOUD_CONTEXT)?;

            let component_name = urldecode(segments[0]);
            if segments.len() == 2 {
                Ok(Self {
                    component_name,
                    worker_name: Some(urldecode(segments[1])),
                })
            } else {
                Ok(Self {
                    component_name,
                    worker_name: None,
                })
            }
        }
    }

    fn to_parts(&self) -> (String, Option<String>) {
        (
            match &self.worker_name {
                Some(worker_name) => Self::make_path2(&self.component_name, worker_name),
                None => Self::make_path1(&self.component_name),
            },
            None,
        )
    }
}

url_from_into!(WorkerUrl);

/// Typed Golem URL for worker function
///
/// Format: `worker:///{component_name}/{worker_name}/{function}`
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct WorkerFunctionUrl {
    pub component_name: String,
    pub worker_name: String,
    pub function: String,
}

impl TypedGolemUrl for WorkerFunctionUrl {
    fn resource_type() -> &'static str {
        WORKER_TYPE_NAME
    }

    fn try_from_parts(path: &str, query: Option<&str>) -> Result<Self, GolemUrlTransformError>
    where
        Self: Sized,
    {
        let (component_name, worker_name, function) = Self::expect_path3(path)?;

        Self::expect_empty_query(query, CLOUD_CONTEXT)?;

        Ok(Self {
            component_name,
            worker_name,
            function,
        })
    }

    fn to_parts(&self) -> (String, Option<String>) {
        (
            Self::make_path3(&self.component_name, &self.worker_name, &self.function),
            None,
        )
    }
}

url_from_into!(WorkerFunctionUrl);

/// Typed Golem URL for worker or worker function
///
/// Format: `worker:///{component_name}/{worker_name}` or `worker:///{component_name}/{worker_name}/{function}`
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum WorkerOrFunctionUrl {
    Worker(WorkerUrl),
    Function(WorkerFunctionUrl),
}

impl TypedGolemUrl for WorkerOrFunctionUrl {
    fn resource_type() -> &'static str {
        WORKER_TYPE_NAME
    }

    fn try_from_parts(path: &str, query: Option<&str>) -> Result<Self, GolemUrlTransformError>
    where
        Self: Sized,
    {
        let has_function = if let Some(rest) = path.strip_prefix('/') {
            if let Some((_, rest)) = rest.split_once('/') {
                rest.contains('/')
            } else {
                false
            }
        } else {
            false
        };

        if has_function {
            Ok(Self::Function(WorkerFunctionUrl::try_from_parts(
                path, query,
            )?))
        } else {
            Ok(Self::Worker(WorkerUrl::try_from_parts(path, query)?))
        }
    }

    fn to_parts(&self) -> (String, Option<String>) {
        match self {
            Self::Worker(w) => w.to_parts(),
            Self::Function(f) => f.to_parts(),
        }
    }
}

url_from_into!(WorkerOrFunctionUrl);

/// Typed Golem URL for API definition
///
/// Format: `api-definition:///{name}/{version}`
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ApiDefinitionUrl {
    pub name: String,
    pub version: String,
}

impl TypedGolemUrl for ApiDefinitionUrl {
    fn resource_type() -> &'static str {
        API_DEFINITION_TYPE_NAME
    }

    fn try_from_parts(path: &str, query: Option<&str>) -> Result<Self, GolemUrlTransformError>
    where
        Self: Sized,
    {
        let (name, version) = Self::expect_path2(path)?;

        Self::expect_empty_query(query, CLOUD_CONTEXT)?;

        Ok(Self { name, version })
    }

    fn to_parts(&self) -> (String, Option<String>) {
        (Self::make_path2(&self.name, &self.version), None)
    }
}

url_from_into!(ApiDefinitionUrl);

/// Typed Golem URL for API deployment
///
/// Format: `api-deployment:///{site}`
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ApiDeploymentUrl {
    pub site: String,
}

impl TypedGolemUrl for ApiDeploymentUrl {
    fn resource_type() -> &'static str {
        API_DEPLOYMENT_TYPE_NAME
    }

    fn try_from_parts(path: &str, query: Option<&str>) -> Result<Self, GolemUrlTransformError>
    where
        Self: Sized,
    {
        let site = Self::expect_path1(path)?;

        Self::expect_empty_query(query, CLOUD_CONTEXT)?;

        Ok(Self { site })
    }

    fn to_parts(&self) -> (String, Option<String>) {
        (Self::make_path1(&self.site), None)
    }
}

url_from_into!(ApiDeploymentUrl);

/// Any valid URL for a known Golem resource
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ResourceUrl {
    Component(ComponentUrl),
    ComponentVersion(ComponentVersionUrl),
    Worker(WorkerUrl),
    WorkerFunction(WorkerFunctionUrl),
    ApiDefinition(ApiDefinitionUrl),
    ApiDeployment(ApiDeploymentUrl),
}

impl TryFrom<&GolemUrl> for ResourceUrl {
    type Error = GolemUrlTransformError;

    fn try_from(value: &GolemUrl) -> Result<Self, Self::Error> {
        match value.resource_type.as_str() {
            COMPONENT_TYPE_NAME => match ComponentOrVersionUrl::try_from(value)? {
                ComponentOrVersionUrl::Component(c) => Ok(ResourceUrl::Component(c)),
                ComponentOrVersionUrl::Version(v) => Ok(ResourceUrl::ComponentVersion(v)),
            },
            WORKER_TYPE_NAME => match WorkerOrFunctionUrl::try_from(value)? {
                WorkerOrFunctionUrl::Worker(w) => Ok(ResourceUrl::Worker(w)),
                WorkerOrFunctionUrl::Function(f) => Ok(ResourceUrl::WorkerFunction(f)),
            },
            API_DEFINITION_TYPE_NAME => Ok(ResourceUrl::ApiDefinition(ApiDefinitionUrl::try_from(
                value,
            )?)),
            API_DEPLOYMENT_TYPE_NAME => Ok(ResourceUrl::ApiDeployment(ApiDeploymentUrl::try_from(
                value,
            )?)),
            typ => Err(GolemUrlTransformError::UnexpectedType {
                expected_types: vec![
                    COMPONENT_TYPE_NAME,
                    WORKER_TYPE_NAME,
                    API_DEFINITION_TYPE_NAME,
                    API_DEPLOYMENT_TYPE_NAME,
                ],
                actual_type: typ.to_string(),
            }),
        }
    }
}

impl TryFrom<GolemUrl> for ResourceUrl {
    type Error = GolemUrlTransformError;

    fn try_from(value: GolemUrl) -> Result<Self, Self::Error> {
        ResourceUrl::try_from(&value)
    }
}

impl From<&ResourceUrl> for GolemUrl {
    fn from(value: &ResourceUrl) -> Self {
        match value {
            ResourceUrl::Component(c) => c.into(),
            ResourceUrl::ComponentVersion(v) => v.into(),
            ResourceUrl::Worker(w) => w.into(),
            ResourceUrl::WorkerFunction(f) => f.into(),
            ResourceUrl::ApiDefinition(d) => d.into(),
            ResourceUrl::ApiDeployment(d) => d.into(),
        }
    }
}

impl From<ResourceUrl> for GolemUrl {
    fn from(value: ResourceUrl) -> Self {
        GolemUrl::from(&value)
    }
}

impl FromStr for ResourceUrl {
    type Err = GolemUrlTransformError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let url =
            GolemUrl::from_str(s).map_err(|err| GolemUrlTransformError::UrlParseError { err })?;

        url.try_into()
    }
}

impl Display for ResourceUrl {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", GolemUrl::from(self))
    }
}

#[cfg(test)]
mod tests {
    use test_r::test;

    use crate::uri::oss::url::{
        ApiDefinitionUrl, ApiDeploymentUrl, ComponentOrVersionUrl, ComponentUrl,
        ComponentVersionUrl, ResourceUrl, WorkerFunctionUrl, WorkerOrFunctionUrl, WorkerUrl,
    };
    use crate::uri::GolemUrl;
    use std::str::FromStr;

    #[test]
    pub fn component_url_to_url() {
        let typed = ComponentUrl {
            name: "some  name".to_string(),
        };

        let untyped: GolemUrl = typed.into();
        assert_eq!(untyped.to_string(), "component:///some++name");
    }

    #[test]
    pub fn component_url_from_url() {
        let untyped = GolemUrl::from_str("component:///some++name").unwrap();
        let typed: ComponentUrl = untyped.try_into().unwrap();

        assert_eq!(typed.name, "some  name");
    }

    #[test]
    pub fn component_url_from_str() {
        let typed = ComponentUrl::from_str("component:///some++name").unwrap();

        assert_eq!(typed.name, "some  name");
    }

    #[test]
    pub fn component_version_url_to_url() {
        let typed = ComponentVersionUrl {
            name: "some  name".to_string(),
            version: 8,
        };

        let untyped: GolemUrl = typed.into();
        assert_eq!(untyped.to_string(), "component:///some++name/8");
    }

    #[test]
    pub fn component_version_url_from_url() {
        let untyped = GolemUrl::from_str("component:///some++name/8").unwrap();
        let typed: ComponentVersionUrl = untyped.try_into().unwrap();

        assert_eq!(typed.name, "some  name");
        assert_eq!(typed.version, 8);
    }

    #[test]
    pub fn component_version_url_from_str() {
        let typed = ComponentVersionUrl::from_str("component:///some++name/8").unwrap();

        assert_eq!(typed.name, "some  name");
        assert_eq!(typed.version, 8);
    }

    #[test]
    pub fn component_or_version_url_to_url() {
        let typed = ComponentOrVersionUrl::Version(ComponentVersionUrl {
            name: "some  name".to_string(),
            version: 8,
        });

        let untyped: GolemUrl = typed.into();
        assert_eq!(untyped.to_string(), "component:///some++name/8");
    }

    #[test]
    pub fn component_or_version_url_from_url() {
        let untyped_version = GolemUrl::from_str("component:///some++name/8").unwrap();
        let untyped_no_version = GolemUrl::from_str("component:///some++name").unwrap();
        let typed_version: ComponentOrVersionUrl = untyped_version.try_into().unwrap();
        let typed_no_version: ComponentOrVersionUrl = untyped_no_version.try_into().unwrap();

        assert_eq!(typed_version.to_string(), "component:///some++name/8");
        assert_eq!(typed_no_version.to_string(), "component:///some++name");
    }

    #[test]
    pub fn component_or_version_url_from_str() {
        let typed_version = ComponentOrVersionUrl::from_str("component:///some++name/8").unwrap();
        let typed_no_version = ComponentOrVersionUrl::from_str("component:///some++name").unwrap();

        assert_eq!(typed_version.to_string(), "component:///some++name/8");
        assert_eq!(typed_no_version.to_string(), "component:///some++name");
    }

    #[test]
    pub fn worker_url_to_url() {
        let typed = WorkerUrl {
            component_name: "my component".to_string(),
            worker_name: Some("my worker".to_string()),
        };

        let untyped: GolemUrl = typed.into();
        assert_eq!(untyped.to_string(), "worker:///my+component/my+worker");
    }

    #[test]
    pub fn worker_url_to_url_no_name() {
        let typed = WorkerUrl {
            component_name: "my component".to_string(),
            worker_name: None,
        };

        let untyped: GolemUrl = typed.into();
        assert_eq!(untyped.to_string(), "worker:///my+component");
    }

    #[test]
    pub fn worker_url_from_url() {
        let untyped = GolemUrl::from_str("worker:///my+component/my+worker").unwrap();
        let typed: WorkerUrl = untyped.try_into().unwrap();

        assert_eq!(typed.component_name, "my component");
        assert_eq!(typed.worker_name, Some("my worker".to_string()));
    }

    #[test]
    pub fn worker_url_from_url_no_name() {
        let untyped = GolemUrl::from_str("worker:///my+component").unwrap();
        let typed: WorkerUrl = untyped.try_into().unwrap();

        assert_eq!(typed.component_name, "my component");
        assert_eq!(typed.worker_name, None);
    }

    #[test]
    pub fn worker_url_from_str() {
        let typed = WorkerUrl::from_str("worker:///my+component/my+worker").unwrap();

        assert_eq!(typed.component_name, "my component");
        assert_eq!(typed.worker_name, Some("my worker".to_string()));
    }

    #[test]
    pub fn worker_url_from_str_no_name() {
        let typed = WorkerUrl::from_str("worker:///my+component").unwrap();

        assert_eq!(typed.component_name, "my component");
        assert_eq!(typed.worker_name, None);
    }

    #[test]
    pub fn worker_function_url_to_url() {
        let typed = WorkerFunctionUrl {
            component_name: "my component".to_string(),
            worker_name: "my worker".to_string(),
            function: "fn a".to_string(),
        };

        let untyped: GolemUrl = typed.into();
        assert_eq!(untyped.to_string(), "worker:///my+component/my+worker/fn+a");
    }

    #[test]
    pub fn worker_function_url_from_url() {
        let untyped = GolemUrl::from_str("worker:///my+component/my+worker/fn+a").unwrap();
        let typed: WorkerFunctionUrl = untyped.try_into().unwrap();

        assert_eq!(typed.component_name, "my component");
        assert_eq!(typed.worker_name, "my worker");
        assert_eq!(typed.function, "fn a");
    }

    #[test]
    pub fn worker_function_url_from_str() {
        let typed = WorkerFunctionUrl::from_str("worker:///my+component/my+worker/fn+a").unwrap();

        assert_eq!(typed.component_name, "my component");
        assert_eq!(typed.worker_name, "my worker");
        assert_eq!(typed.function, "fn a");
    }

    #[test]
    pub fn worker_or_function_url_to_url() {
        let typed_w = WorkerOrFunctionUrl::Worker(WorkerUrl {
            component_name: "my component".to_string(),
            worker_name: Some("my worker".to_string()),
        });
        let typed_f = WorkerOrFunctionUrl::Function(WorkerFunctionUrl {
            component_name: "my component".to_string(),
            worker_name: "my worker".to_string(),
            function: "fn a".to_string(),
        });

        let untyped_w: GolemUrl = typed_w.into();
        let untyped_f: GolemUrl = typed_f.into();

        assert_eq!(untyped_w.to_string(), "worker:///my+component/my+worker");
        assert_eq!(
            untyped_f.to_string(),
            "worker:///my+component/my+worker/fn+a"
        );
    }

    #[test]
    pub fn worker_or_function_url_from_url() {
        let untyped_w = GolemUrl::from_str("worker:///my+component/my+worker").unwrap();
        let untyped_f = GolemUrl::from_str("worker:///my+component/my+worker/fn+a").unwrap();

        let typed_w: WorkerOrFunctionUrl = untyped_w.try_into().unwrap();
        let typed_f: WorkerOrFunctionUrl = untyped_f.try_into().unwrap();

        assert_eq!(typed_w.to_string(), "worker:///my+component/my+worker");
        assert_eq!(typed_f.to_string(), "worker:///my+component/my+worker/fn+a");
    }

    #[test]
    pub fn worker_or_function_url_from_str() {
        let typed_w = WorkerOrFunctionUrl::from_str("worker:///my+component/my+worker").unwrap();
        let typed_f =
            WorkerOrFunctionUrl::from_str("worker:///my+component/my+worker/fn+a").unwrap();

        assert_eq!(typed_w.to_string(), "worker:///my+component/my+worker");
        assert_eq!(typed_f.to_string(), "worker:///my+component/my+worker/fn+a");
    }

    #[test]
    pub fn api_definition_url_to_url() {
        let typed = ApiDefinitionUrl {
            name: "my def".to_string(),
            version: "1.2.3".to_string(),
        };

        let untyped: GolemUrl = typed.into();
        assert_eq!(untyped.to_string(), "api-definition:///my+def/1.2.3");
    }

    #[test]
    pub fn api_definition_url_from_url() {
        let untyped = GolemUrl::from_str("api-definition:///my+def/1.2.3").unwrap();
        let typed: ApiDefinitionUrl = untyped.try_into().unwrap();

        assert_eq!(typed.name, "my def");
        assert_eq!(typed.version, "1.2.3");
    }

    #[test]
    pub fn api_definition_url_from_str() {
        let typed = ApiDefinitionUrl::from_str("api-definition:///my+def/1.2.3").unwrap();

        assert_eq!(typed.name, "my def");
        assert_eq!(typed.version, "1.2.3");
    }

    #[test]
    pub fn api_deployment_url_to_url() {
        let typed = ApiDeploymentUrl {
            site: "example.com".to_string(),
        };

        let untyped: GolemUrl = typed.into();
        assert_eq!(untyped.to_string(), "api-deployment:///example.com");
    }

    #[test]
    pub fn api_deployment_url_from_url() {
        let untyped = GolemUrl::from_str("api-deployment:///example.com").unwrap();
        let typed: ApiDeploymentUrl = untyped.try_into().unwrap();

        assert_eq!(typed.site, "example.com");
    }

    #[test]
    pub fn api_deployment_url_from_str() {
        let typed = ApiDeploymentUrl::from_str("api-deployment:///example.com").unwrap();

        assert_eq!(typed.site, "example.com");
    }

    #[test]
    pub fn resource_url_from_url() {
        let untyped_cv = GolemUrl::from_str("component:///comp_name/11").unwrap();
        let untyped_ad = GolemUrl::from_str("api-deployment:///example.com").unwrap();
        let typed_cv: ResourceUrl = untyped_cv.try_into().unwrap();
        let typed_ad: ResourceUrl = untyped_ad.try_into().unwrap();

        assert_eq!(
            GolemUrl::from(typed_cv).to_string(),
            "component:///comp_name/11"
        );
        assert_eq!(
            GolemUrl::from(typed_ad).to_string(),
            "api-deployment:///example.com"
        );
    }

    #[test]
    pub fn resource_url_from_str() {
        let typed_cv = ResourceUrl::from_str("component:///comp_name/11").unwrap();
        let typed_ad = ResourceUrl::from_str("api-deployment:///example.com").unwrap();

        assert_eq!(typed_cv.to_string(), "component:///comp_name/11");
        assert_eq!(typed_ad.to_string(), "api-deployment:///example.com");
    }
}
