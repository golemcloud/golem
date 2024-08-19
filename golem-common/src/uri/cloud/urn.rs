use crate::model::{AccountId, ProjectId};
use crate::uri::{
    try_from_golem_urn, urldecode, urlencode, GolemUrn, GolemUrnTransformError, TypedGolemUrn,
    API_DEFINITION_TYPE_NAME, API_DEPLOYMENT_TYPE_NAME, COMPONENT_TYPE_NAME, WORKER_TYPE_NAME,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use uuid::Uuid;

use crate::uri::cloud::{ACCOUNT_TYPE_NAME, PROJECT_TYPE_NAME};
pub use crate::uri::oss::urn::ApiDefinitionUrn;
pub use crate::uri::oss::urn::ApiDeploymentUrn;
pub use crate::uri::oss::urn::ComponentOrVersionUrn;
pub use crate::uri::oss::urn::ComponentUrn;
pub use crate::uri::oss::urn::ComponentVersionUrn;
pub use crate::uri::oss::urn::WorkerFunctionUrn;
pub use crate::uri::oss::urn::WorkerOrFunctionUrn;
pub use crate::uri::oss::urn::WorkerUrn;
use crate::urn_from_into;

/// Typed Golem URN for account
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct AccountUrn {
    pub id: AccountId,
}

impl TypedGolemUrn for AccountUrn {
    fn resource_type() -> &'static str {
        ACCOUNT_TYPE_NAME
    }

    fn try_from_name(resource_name: &str) -> Result<Self, GolemUrnTransformError> {
        let id = urldecode(resource_name);

        Ok(Self {
            id: AccountId { value: id },
        })
    }

    fn to_name(&self) -> String {
        urlencode(&self.id.value)
    }
}

urn_from_into!(AccountUrn);

/// Typed Golem URN for project
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ProjectUrn {
    pub id: ProjectId,
}

impl TypedGolemUrn for ProjectUrn {
    fn resource_type() -> &'static str {
        PROJECT_TYPE_NAME
    }

    fn try_from_name(resource_name: &str) -> Result<Self, GolemUrnTransformError> {
        let id = Uuid::parse_str(resource_name).map_err(|err| {
            GolemUrnTransformError::invalid_name(
                Self::resource_type(),
                format!("Can't parse UUID: {err}"),
            )
        })?;

        Ok(Self { id: ProjectId(id) })
    }

    fn to_name(&self) -> String {
        self.id.0.to_string()
    }
}

urn_from_into!(ProjectUrn);

/// Any valid URN for a known Golem resource
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ResourceUrn {
    Account(AccountUrn),
    Project(ProjectUrn),
    Component(ComponentUrn),
    ComponentVersion(ComponentVersionUrn),
    Worker(WorkerUrn),
    WorkerFunction(WorkerFunctionUrn),
    ApiDefinition(ApiDefinitionUrn),
    ApiDeployment(ApiDeploymentUrn),
}

impl TryFrom<&GolemUrn> for ResourceUrn {
    type Error = GolemUrnTransformError;

    fn try_from(value: &GolemUrn) -> Result<Self, Self::Error> {
        match value.resource_type.as_str() {
            ACCOUNT_TYPE_NAME => Ok(ResourceUrn::Account(AccountUrn::try_from(value)?)),
            PROJECT_TYPE_NAME => Ok(ResourceUrn::Project(ProjectUrn::try_from(value)?)),
            COMPONENT_TYPE_NAME => match ComponentOrVersionUrn::try_from(value)? {
                ComponentOrVersionUrn::Component(c) => Ok(ResourceUrn::Component(c)),
                ComponentOrVersionUrn::Version(v) => Ok(ResourceUrn::ComponentVersion(v)),
            },
            WORKER_TYPE_NAME => match WorkerOrFunctionUrn::try_from(value)? {
                WorkerOrFunctionUrn::Worker(w) => Ok(ResourceUrn::Worker(w)),
                WorkerOrFunctionUrn::Function(f) => Ok(ResourceUrn::WorkerFunction(f)),
            },
            API_DEFINITION_TYPE_NAME => Ok(ResourceUrn::ApiDefinition(ApiDefinitionUrn::try_from(
                value,
            )?)),
            API_DEPLOYMENT_TYPE_NAME => Ok(ResourceUrn::ApiDeployment(ApiDeploymentUrn::try_from(
                value,
            )?)),
            typ => Err(GolemUrnTransformError::UnexpectedType {
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

impl TryFrom<GolemUrn> for ResourceUrn {
    type Error = GolemUrnTransformError;

    fn try_from(value: GolemUrn) -> Result<Self, Self::Error> {
        ResourceUrn::try_from(&value)
    }
}

impl From<&ResourceUrn> for GolemUrn {
    fn from(value: &ResourceUrn) -> Self {
        match value {
            ResourceUrn::Account(a) => a.into(),
            ResourceUrn::Project(p) => p.into(),
            ResourceUrn::Component(c) => c.into(),
            ResourceUrn::ComponentVersion(v) => v.into(),
            ResourceUrn::Worker(w) => w.into(),
            ResourceUrn::WorkerFunction(f) => f.into(),
            ResourceUrn::ApiDefinition(d) => d.into(),
            ResourceUrn::ApiDeployment(d) => d.into(),
        }
    }
}

impl From<ResourceUrn> for GolemUrn {
    fn from(value: ResourceUrn) -> Self {
        GolemUrn::from(&value)
    }
}

impl FromStr for ResourceUrn {
    type Err = GolemUrnTransformError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let urn =
            GolemUrn::from_str(s).map_err(|err| GolemUrnTransformError::UrnParseError { err })?;

        urn.try_into()
    }
}

impl Display for ResourceUrn {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", GolemUrn::from(self))
    }
}

#[cfg(test)]
mod tests {
    use crate::model::{AccountId, ProjectId};
    use crate::uri::cloud::urn::{AccountUrn, ProjectUrn, ResourceUrn};
    use crate::uri::GolemUrn;
    use std::str::FromStr;
    use uuid::Uuid;

    #[test]
    pub fn account_urn_to_urn() {
        let typed = AccountUrn {
            id: AccountId {
                value: "acc".to_string(),
            },
        };

        let untyped: GolemUrn = typed.into();
        assert_eq!(untyped.to_string(), "urn:account:acc");
    }

    #[test]
    pub fn account_urn_from_urn() {
        let untyped = GolemUrn::from_str("urn:account:acc").unwrap();
        let typed: AccountUrn = untyped.try_into().unwrap();

        assert_eq!(typed.id.value, "acc");
    }

    #[test]
    pub fn account_urn_from_str() {
        let typed = AccountUrn::from_str("urn:account:acc").unwrap();

        assert_eq!(typed.id.value, "acc");
    }

    #[test]
    pub fn project_urn_to_urn() {
        let typed = ProjectUrn {
            id: ProjectId(Uuid::parse_str("679ae459-8700-41d9-920c-7e2887459c94").unwrap()),
        };

        let untyped: GolemUrn = typed.into();
        assert_eq!(
            untyped.to_string(),
            "urn:project:679ae459-8700-41d9-920c-7e2887459c94"
        );
    }

    #[test]
    pub fn project_urn_from_urn() {
        let untyped =
            GolemUrn::from_str("urn:project:679ae459-8700-41d9-920c-7e2887459c94").unwrap();
        let typed: ProjectUrn = untyped.try_into().unwrap();

        assert_eq!(
            typed.id.0.to_string(),
            "679ae459-8700-41d9-920c-7e2887459c94"
        );
    }

    #[test]
    pub fn project_urn_from_str() {
        let typed =
            ProjectUrn::from_str("urn:project:679ae459-8700-41d9-920c-7e2887459c94").unwrap();

        assert_eq!(
            typed.id.0.to_string(),
            "679ae459-8700-41d9-920c-7e2887459c94"
        );
    }

    #[test]
    pub fn resource_urn_from_urn() {
        let untyped_p =
            GolemUrn::from_str("urn:project:679ae459-8700-41d9-920c-7e2887459c94").unwrap();
        let untyped_a = GolemUrn::from_str("urn:account:acc").unwrap();
        let typed_p: ResourceUrn = untyped_p.try_into().unwrap();
        let typed_a: ResourceUrn = untyped_a.try_into().unwrap();

        assert_eq!(
            GolemUrn::from(typed_p).to_string(),
            "urn:project:679ae459-8700-41d9-920c-7e2887459c94"
        );
        assert_eq!(GolemUrn::from(typed_a).to_string(), "urn:account:acc");
    }

    #[test]
    pub fn resource_urn_from_str() {
        let typed_p =
            ResourceUrn::from_str("urn:project:679ae459-8700-41d9-920c-7e2887459c94").unwrap();
        let typed_a = ResourceUrn::from_str("urn:account:acc").unwrap();

        assert_eq!(
            typed_p.to_string(),
            "urn:project:679ae459-8700-41d9-920c-7e2887459c94"
        );
        assert_eq!(typed_a.to_string(), "urn:account:acc");
    }
}
