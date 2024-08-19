use crate::model::ComponentVersion;
use crate::uri::cloud::{ACCOUNT_TYPE_NAME, PROJECT_TYPE_NAME};
use crate::uri::{
    try_from_golem_url, urlencode, GolemUrl, GolemUrlTransformError, TypedGolemUrl,
    API_DEFINITION_TYPE_NAME, API_DEPLOYMENT_TYPE_NAME, COMPONENT_TYPE_NAME, WORKER_TYPE_NAME,
};
use crate::url_from_into;
use itertools::Itertools;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt::{Display, Formatter};
use std::str::FromStr;

/// Typed Golem URL for account
///
/// Format with optional account: `account:///{name}`
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AccountUrl {
    pub name: String,
}

impl TypedGolemUrl for AccountUrl {
    fn resource_type() -> &'static str {
        ACCOUNT_TYPE_NAME
    }

    fn try_from_parts(path: &str, query: Option<&str>) -> Result<Self, GolemUrlTransformError>
    where
        Self: Sized,
    {
        let name = Self::expect_path1(path)?;

        let _ = Self::expect_query(query, &[])?;

        Ok(Self { name })
    }

    fn to_parts(&self) -> (String, Option<String>) {
        (Self::make_path1(&self.name), None)
    }
}

url_from_into!(AccountUrl);

/// Typed Golem URL for project
///
/// Format with optional account: `project:///{name}?account={account_name}`
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ProjectUrl {
    pub name: String,
    pub account: Option<AccountUrl>,
}

impl TypedGolemUrl for ProjectUrl {
    fn resource_type() -> &'static str {
        PROJECT_TYPE_NAME
    }

    fn try_from_parts(path: &str, query: Option<&str>) -> Result<Self, GolemUrlTransformError>
    where
        Self: Sized,
    {
        let name = Self::expect_path1(path)?;
        let mut query = Self::expect_query(query, &[ACCOUNT_TYPE_NAME])?;
        let account = query
            .remove(&ACCOUNT_TYPE_NAME)
            .map(|account_name| AccountUrl { name: account_name });

        Ok(Self { name, account })
    }

    fn to_parts(&self) -> (String, Option<String>) {
        let query = self
            .account
            .as_ref()
            .map(|AccountUrl { name: account_name }| {
                format!("{ACCOUNT_TYPE_NAME}={}", urlencode(account_name))
            });
        (Self::make_path1(&self.name), query)
    }
}

url_from_into!(ProjectUrl);

pub trait ToOssUrl {
    type Target;
    fn to_oss_url(self) -> (Self::Target, Option<ProjectUrl>);
}

/// Typed Golem URL for component
///
/// Format with optional project and account: `component:///{name}?account={account_name}&project={project_name}`
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ComponentUrl {
    pub name: String,
    pub project: Option<ProjectUrl>,
}

impl ToOssUrl for ComponentUrl {
    type Target = crate::uri::oss::url::ComponentUrl;

    fn to_oss_url(self) -> (Self::Target, Option<ProjectUrl>) {
        (
            crate::uri::oss::url::ComponentUrl { name: self.name },
            self.project,
        )
    }
}

fn to_project_query(project: &Option<ProjectUrl>) -> Option<String> {
    match project {
        None => None,
        Some(ProjectUrl {
            name: project_name,
            account,
        }) => {
            let account_part = account.as_ref().map(|AccountUrl { name: account_name }| {
                format!("{ACCOUNT_TYPE_NAME}={}", urlencode(account_name))
            });

            let project_part = Some(format!("{PROJECT_TYPE_NAME}={}", urlencode(project_name)));

            Some([account_part, project_part].into_iter().flatten().join("&"))
        }
    }
}

impl TypedGolemUrl for ComponentUrl {
    fn resource_type() -> &'static str {
        COMPONENT_TYPE_NAME
    }

    fn try_from_parts(path: &str, query: Option<&str>) -> Result<Self, GolemUrlTransformError>
    where
        Self: Sized,
    {
        let name = Self::expect_path1(path)?;
        let project = Self::expect_project_query(query)?;

        Ok(Self { name, project })
    }

    fn to_parts(&self) -> (String, Option<String>) {
        (
            Self::make_path1(&self.name),
            to_project_query(&self.project),
        )
    }
}

url_from_into!(ComponentUrl);

/// Typed Golem URL for component version
///
/// Format with optional project and account: `component:///{name}/{version}?account={account_name}&project={project_name}`
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ComponentVersionUrl {
    pub name: String,
    pub version: ComponentVersion,
    pub project: Option<ProjectUrl>,
}

impl ToOssUrl for ComponentVersionUrl {
    type Target = crate::uri::oss::url::ComponentVersionUrl;

    fn to_oss_url(self) -> (Self::Target, Option<ProjectUrl>) {
        (
            crate::uri::oss::url::ComponentVersionUrl {
                name: self.name,
                version: self.version,
            },
            self.project,
        )
    }
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

        let project = Self::expect_project_query(query)?;

        Ok(Self {
            name,
            version,
            project,
        })
    }

    fn to_parts(&self) -> (String, Option<String>) {
        (
            Self::make_path2(&self.name, &self.version.to_string()),
            to_project_query(&self.project),
        )
    }
}

url_from_into!(ComponentVersionUrl);

/// Typed Golem URL for component or component version
///
/// Format with optional project and account: `component:///{name}?account={account_name}&project={project_name}`
/// or `component:///{name}/{version}?account={account_name}&project={project_name}`
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ComponentOrVersionUrl {
    Component(ComponentUrl),
    Version(ComponentVersionUrl),
}

impl ToOssUrl for ComponentOrVersionUrl {
    type Target = crate::uri::oss::url::ComponentOrVersionUrl;

    fn to_oss_url(self) -> (Self::Target, Option<ProjectUrl>) {
        match self {
            ComponentOrVersionUrl::Component(c) => {
                let (c, p) = c.to_oss_url();

                (crate::uri::oss::url::ComponentOrVersionUrl::Component(c), p)
            }
            ComponentOrVersionUrl::Version(v) => {
                let (v, p) = v.to_oss_url();

                (crate::uri::oss::url::ComponentOrVersionUrl::Version(v), p)
            }
        }
    }
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
/// Format with optional project and account:
/// `worker:///{component_name}/{worker_name}?account={account_name}&project={project_name}`
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct WorkerUrl {
    pub component_name: String,
    pub worker_name: String,
    pub project: Option<ProjectUrl>,
}

impl ToOssUrl for WorkerUrl {
    type Target = crate::uri::oss::url::WorkerUrl;

    fn to_oss_url(self) -> (Self::Target, Option<ProjectUrl>) {
        (
            crate::uri::oss::url::WorkerUrl {
                component_name: self.component_name,
                worker_name: self.worker_name,
            },
            self.project,
        )
    }
}

impl TypedGolemUrl for WorkerUrl {
    fn resource_type() -> &'static str {
        WORKER_TYPE_NAME
    }

    fn try_from_parts(path: &str, query: Option<&str>) -> Result<Self, GolemUrlTransformError>
    where
        Self: Sized,
    {
        let (component_name, worker_name) = Self::expect_path2(path)?;

        let project = Self::expect_project_query(query)?;

        Ok(Self {
            component_name,
            worker_name,
            project,
        })
    }

    fn to_parts(&self) -> (String, Option<String>) {
        (
            Self::make_path2(&self.component_name, &self.worker_name),
            to_project_query(&self.project),
        )
    }
}

url_from_into!(WorkerUrl);

/// Typed Golem URL for worker function
///
/// Format with optional project and account:
/// `worker:///{component_name}/{worker_name}/{function}?account={account_name}&project={project_name}`
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct WorkerFunctionUrl {
    pub component_name: String,
    pub worker_name: String,
    pub function: String,
    pub project: Option<ProjectUrl>,
}

impl ToOssUrl for WorkerFunctionUrl {
    type Target = crate::uri::oss::url::WorkerFunctionUrl;

    fn to_oss_url(self) -> (Self::Target, Option<ProjectUrl>) {
        (
            crate::uri::oss::url::WorkerFunctionUrl {
                component_name: self.component_name,
                worker_name: self.worker_name,
                function: self.function,
            },
            self.project,
        )
    }
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

        let project = Self::expect_project_query(query)?;

        Ok(Self {
            component_name,
            worker_name,
            function,
            project,
        })
    }

    fn to_parts(&self) -> (String, Option<String>) {
        (
            Self::make_path3(&self.component_name, &self.worker_name, &self.function),
            to_project_query(&self.project),
        )
    }
}

url_from_into!(WorkerFunctionUrl);

/// Typed Golem URL for worker or worker function
///
/// Format with optional project and account:
/// `worker:///{component_name}/{worker_name}?account={account_name}&project={project_name}`
/// or `worker:///{component_name}/{worker_name}/{function}?account={account_name}&project={project_name}`
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum WorkerOrFunctionUrl {
    Worker(WorkerUrl),
    Function(WorkerFunctionUrl),
}

impl ToOssUrl for WorkerOrFunctionUrl {
    type Target = crate::uri::oss::url::WorkerOrFunctionUrl;

    fn to_oss_url(self) -> (Self::Target, Option<ProjectUrl>) {
        match self {
            WorkerOrFunctionUrl::Worker(w) => {
                let (w, p) = w.to_oss_url();

                (crate::uri::oss::url::WorkerOrFunctionUrl::Worker(w), p)
            }
            WorkerOrFunctionUrl::Function(f) => {
                let (f, p) = f.to_oss_url();

                (crate::uri::oss::url::WorkerOrFunctionUrl::Function(f), p)
            }
        }
    }
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
/// Format with optional project and account:
/// `api-definition:///{name}/{version}?account={account_name}&project={project_name}`
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ApiDefinitionUrl {
    pub name: String,
    pub version: String,
    pub project: Option<ProjectUrl>,
}

impl ToOssUrl for ApiDefinitionUrl {
    type Target = crate::uri::oss::url::ApiDefinitionUrl;

    fn to_oss_url(self) -> (Self::Target, Option<ProjectUrl>) {
        (
            crate::uri::oss::url::ApiDefinitionUrl {
                name: self.name,
                version: self.version,
            },
            self.project,
        )
    }
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

        let project = Self::expect_project_query(query)?;

        Ok(Self {
            name,
            version,
            project,
        })
    }

    fn to_parts(&self) -> (String, Option<String>) {
        (
            Self::make_path2(&self.name, &self.version),
            to_project_query(&self.project),
        )
    }
}

url_from_into!(ApiDefinitionUrl);

/// Typed Golem URL for API deployment
///
/// Format with optional project and account:
/// `api-deployment:///{site}?account={account_name}&project={project_name}`
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ApiDeploymentUrl {
    pub site: String,
    pub project: Option<ProjectUrl>,
}

impl ToOssUrl for ApiDeploymentUrl {
    type Target = crate::uri::oss::url::ApiDeploymentUrl;

    fn to_oss_url(self) -> (Self::Target, Option<ProjectUrl>) {
        (
            crate::uri::oss::url::ApiDeploymentUrl { site: self.site },
            self.project,
        )
    }
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

        let project = Self::expect_project_query(query)?;

        Ok(Self { site, project })
    }

    fn to_parts(&self) -> (String, Option<String>) {
        (
            Self::make_path1(&self.site),
            to_project_query(&self.project),
        )
    }
}

url_from_into!(ApiDeploymentUrl);

/// Any valid URL for a known Golem resource
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ResourceUrl {
    Account(AccountUrl),
    Project(ProjectUrl),
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
            ACCOUNT_TYPE_NAME => Ok(ResourceUrl::Account(AccountUrl::try_from(value)?)),
            PROJECT_TYPE_NAME => Ok(ResourceUrl::Project(ProjectUrl::try_from(value)?)),
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
                    ACCOUNT_TYPE_NAME,
                    PROJECT_TYPE_NAME,
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
            ResourceUrl::Account(a) => a.into(),
            ResourceUrl::Project(p) => p.into(),
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
    use crate::uri::cloud::url::{
        AccountUrl, ApiDefinitionUrl, ApiDeploymentUrl, ComponentOrVersionUrl, ComponentUrl,
        ComponentVersionUrl, ProjectUrl, ResourceUrl, WorkerFunctionUrl, WorkerOrFunctionUrl,
        WorkerUrl,
    };
    use crate::uri::GolemUrl;
    use std::str::FromStr;

    #[test]
    pub fn account_url_to_url() {
        let typed = AccountUrl {
            name: "acc".to_string(),
        };

        let untyped: GolemUrl = typed.into();
        assert_eq!(untyped.to_string(), "account:///acc");
    }

    #[test]
    pub fn account_url_from_url() {
        let untyped = GolemUrl::from_str("account:///acc").unwrap();
        let typed: AccountUrl = untyped.try_into().unwrap();

        assert_eq!(typed.name, "acc");
    }

    #[test]
    pub fn account_url_from_str() {
        let typed = AccountUrl::from_str("account:///acc").unwrap();

        assert_eq!(typed.name, "acc");
    }

    #[test]
    pub fn project_url_to_url() {
        let typed = ProjectUrl {
            name: "proj".to_string(),
            account: None,
        };

        let untyped: GolemUrl = typed.into();
        assert_eq!(untyped.to_string(), "project:///proj");
    }

    #[test]
    pub fn project_url_from_url() {
        let untyped = GolemUrl::from_str("project:///proj").unwrap();
        let typed: ProjectUrl = untyped.try_into().unwrap();

        assert_eq!(typed.name, "proj");
    }

    #[test]
    pub fn project_url_from_str() {
        let typed = ProjectUrl::from_str("project:///proj").unwrap();

        assert_eq!(typed.name, "proj");
    }

    #[test]
    pub fn component_url_to_url() {
        let typed = ComponentUrl {
            name: "some  name".to_string(),
            project: None,
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
            project: None,
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
            project: None,
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
            worker_name: "my worker".to_string(),
            project: None,
        };

        let untyped: GolemUrl = typed.into();
        assert_eq!(untyped.to_string(), "worker:///my+component/my+worker");
    }

    #[test]
    pub fn worker_url_from_url() {
        let untyped = GolemUrl::from_str("worker:///my+component/my+worker").unwrap();
        let typed: WorkerUrl = untyped.try_into().unwrap();

        assert_eq!(typed.component_name, "my component");
        assert_eq!(typed.worker_name, "my worker");
    }

    #[test]
    pub fn worker_url_from_str() {
        let typed = WorkerUrl::from_str("worker:///my+component/my+worker").unwrap();

        assert_eq!(typed.component_name, "my component");
        assert_eq!(typed.worker_name, "my worker");
    }

    #[test]
    pub fn worker_function_url_to_url() {
        let typed = WorkerFunctionUrl {
            component_name: "my component".to_string(),
            worker_name: "my worker".to_string(),
            function: "fn a".to_string(),
            project: None,
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
            worker_name: "my worker".to_string(),
            project: None,
        });
        let typed_f = WorkerOrFunctionUrl::Function(WorkerFunctionUrl {
            component_name: "my component".to_string(),
            worker_name: "my worker".to_string(),
            function: "fn a".to_string(),
            project: None,
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
            project: None,
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
            project: None,
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
        let typed_a = ResourceUrl::from_str("account:///acc").unwrap();
        let typed_p = ResourceUrl::from_str("project:///proj").unwrap();

        assert_eq!(typed_a.to_string(), "account:///acc");
        assert_eq!(typed_p.to_string(), "project:///proj");
    }

    #[test]
    pub fn resource_url_context() {
        let typed_p = ResourceUrl::from_str("project:///proj?account=acc").unwrap();
        let typed_c = ResourceUrl::from_str("component:///comp?account=acc&project=proj").unwrap();
        let typed_c2 = ResourceUrl::from_str("component:///comp?project=proj").unwrap();
        let typed_cv =
            ResourceUrl::from_str("component:///comp/1?account=acc&project=proj").unwrap();
        let typed_w = ResourceUrl::from_str("worker:///comp/w?account=acc&project=proj").unwrap();
        let typed_f = ResourceUrl::from_str("worker:///comp/w/f?account=acc&project=proj").unwrap();
        let typed_def =
            ResourceUrl::from_str("api-definition:///def/1.2.3?account=acc&project=proj").unwrap();
        let typed_dep =
            ResourceUrl::from_str("api-deployment:///example.com?account=acc&project=proj")
                .unwrap();

        assert_eq!(typed_p.to_string(), "project:///proj?account=acc");
        assert_eq!(
            typed_c.to_string(),
            "component:///comp?account=acc&project=proj"
        );
        assert_eq!(typed_c2.to_string(), "component:///comp?project=proj");
        assert_eq!(
            typed_cv.to_string(),
            "component:///comp/1?account=acc&project=proj"
        );
        assert_eq!(
            typed_w.to_string(),
            "worker:///comp/w?account=acc&project=proj"
        );
        assert_eq!(
            typed_f.to_string(),
            "worker:///comp/w/f?account=acc&project=proj"
        );
        assert_eq!(
            typed_def.to_string(),
            "api-definition:///def/1.2.3?account=acc&project=proj"
        );
        assert_eq!(
            typed_dep.to_string(),
            "api-deployment:///example.com?account=acc&project=proj"
        );
    }
}
