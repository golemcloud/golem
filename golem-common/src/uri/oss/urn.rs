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

use crate::model::{ComponentId, ComponentVersion, TargetWorkerId, WorkerId};
use crate::uri::{
    try_from_golem_urn, urldecode, urlencode, GolemUrn, GolemUrnTransformError, TypedGolemUrn,
    API_DEFINITION_TYPE_NAME, API_DEPLOYMENT_TYPE_NAME, COMPONENT_TYPE_NAME, WORKER_TYPE_NAME,
};
use crate::urn_from_into;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt::{Display, Formatter};
use std::str::FromStr;
use uuid::Uuid;

/// Typed Golem URN for component
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ComponentUrn {
    pub id: ComponentId,
}

impl TypedGolemUrn for ComponentUrn {
    fn resource_type() -> &'static str {
        COMPONENT_TYPE_NAME
    }

    fn try_from_name(resource_name: &str) -> Result<Self, GolemUrnTransformError> {
        let id = Uuid::parse_str(resource_name).map_err(|err| {
            GolemUrnTransformError::invalid_name(
                Self::resource_type(),
                format!("Can't parse UUID: {err}"),
            )
        })?;

        Ok(Self {
            id: ComponentId(id),
        })
    }

    fn to_name(&self) -> String {
        self.id.0.to_string()
    }
}

urn_from_into!(ComponentUrn);

/// Typed Golem URN for component version
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ComponentVersionUrn {
    pub id: ComponentId,
    pub version: ComponentVersion,
}

impl TypedGolemUrn for ComponentVersionUrn {
    fn resource_type() -> &'static str {
        COMPONENT_TYPE_NAME
    }

    fn try_from_name(resource_name: &str) -> Result<Self, GolemUrnTransformError> {
        if let Some((id, version)) = resource_name.split_once('/') {
            let id = Uuid::parse_str(id).map_err(|err| {
                GolemUrnTransformError::invalid_name(
                    Self::resource_type(),
                    format!("Can't parse UUID: {err}"),
                )
            })?;
            let version: ComponentVersion = version.parse().map_err(|err| {
                GolemUrnTransformError::invalid_name(
                    Self::resource_type(),
                    format!("Can't parse component version: {err}"),
                )
            })?;

            Ok(ComponentVersionUrn {
                id: ComponentId(id),
                version,
            })
        } else {
            Err(GolemUrnTransformError::invalid_name(
                Self::resource_type(),
                "Component version expected".to_string(),
            ))
        }
    }

    fn to_name(&self) -> String {
        format!("{}/{}", self.id.0, self.version)
    }
}

urn_from_into!(ComponentVersionUrn);

/// Typed Golem URN for component or component version
///
/// It can be used as component with optional version.
/// Absent version can be used to represent the current version.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ComponentOrVersionUrn {
    Component(ComponentUrn),
    Version(ComponentVersionUrn),
}

impl TypedGolemUrn for ComponentOrVersionUrn {
    fn resource_type() -> &'static str {
        COMPONENT_TYPE_NAME
    }

    fn try_from_name(resource_name: &str) -> Result<Self, GolemUrnTransformError> {
        if resource_name.contains('/') {
            let res = ComponentVersionUrn::try_from_name(resource_name)?;

            Ok(Self::Version(res))
        } else {
            let res = ComponentUrn::try_from_name(resource_name)?;

            Ok(Self::Component(res))
        }
    }

    fn to_name(&self) -> String {
        match self {
            ComponentOrVersionUrn::Component(c) => c.to_name(),
            ComponentOrVersionUrn::Version(v) => v.to_name(),
        }
    }
}

urn_from_into!(ComponentOrVersionUrn);

/// Typed Golem URN for worker
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct WorkerUrn {
    pub id: TargetWorkerId,
}

impl WorkerUrn {
    pub fn worker_id(&self) -> Result<WorkerId, GolemUrnTransformError> {
        match &self.id.worker_name {
            Some(name) => Ok(WorkerId {
                component_id: self.id.component_id.clone(),
                worker_name: name.clone(),
            }),
            None => Err(GolemUrnTransformError::invalid_name(
                Self::resource_type(),
                "Worker name expected".to_string(),
            )),
        }
    }
}

impl TypedGolemUrn for WorkerUrn {
    fn resource_type() -> &'static str {
        WORKER_TYPE_NAME
    }

    fn try_from_name(resource_name: &str) -> Result<Self, GolemUrnTransformError> {
        if let Some((id, worker_name)) = resource_name.split_once('/') {
            let id = Uuid::parse_str(id).map_err(|err| {
                GolemUrnTransformError::invalid_name(
                    Self::resource_type(),
                    format!("Can't parse UUID: {err}"),
                )
            })?;

            let worker_name = urldecode(worker_name);

            Ok(Self {
                id: TargetWorkerId {
                    component_id: ComponentId(id),
                    worker_name: Some(worker_name),
                },
            })
        } else {
            let id = Uuid::parse_str(resource_name).map_err(|err| {
                GolemUrnTransformError::invalid_name(
                    Self::resource_type(),
                    format!("Can't parse UUID: {err}"),
                )
            })?;

            Ok(Self {
                id: TargetWorkerId {
                    component_id: ComponentId(id),
                    worker_name: None,
                },
            })
        }
    }

    fn to_name(&self) -> String {
        match self.id.worker_name {
            Some(ref worker_name) => {
                format!("{}/{}", self.id.component_id.0, urlencode(worker_name))
            }
            None => self.id.component_id.0.to_string(),
        }
    }
}

urn_from_into!(WorkerUrn);

/// Typed Golem URN for worker function
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct WorkerFunctionUrn {
    pub id: WorkerId,
    pub function: String,
}

impl TypedGolemUrn for WorkerFunctionUrn {
    fn resource_type() -> &'static str {
        WORKER_TYPE_NAME
    }

    fn try_from_name(resource_name: &str) -> Result<Self, GolemUrnTransformError> {
        if let Some((id, rest)) = resource_name.split_once('/') {
            let id = Uuid::parse_str(id).map_err(|err| {
                GolemUrnTransformError::invalid_name(
                    Self::resource_type(),
                    format!("Can't parse UUID: {err}"),
                )
            })?;

            if let Some((worker_name, function)) = rest.split_once('/') {
                let worker_name = urldecode(worker_name);
                let function = urldecode(function);

                Ok(Self {
                    id: WorkerId {
                        component_id: ComponentId(id),
                        worker_name,
                    },
                    function,
                })
            } else {
                Err(GolemUrnTransformError::invalid_name(
                    Self::resource_type(),
                    "Function name expected".to_string(),
                ))
            }
        } else {
            Err(GolemUrnTransformError::invalid_name(
                Self::resource_type(),
                "Worker name expected".to_string(),
            ))
        }
    }

    fn to_name(&self) -> String {
        format!(
            "{}/{}/{}",
            self.id.component_id.0,
            urlencode(&self.id.worker_name),
            urlencode(&self.function),
        )
    }
}

urn_from_into!(WorkerFunctionUrn);

/// Typed Golem URN for worker or worker function
///
/// It can be used as worker with optional function name.
/// Used in RPC.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum WorkerOrFunctionUrn {
    Worker(WorkerUrn),
    Function(WorkerFunctionUrn),
}

impl TypedGolemUrn for WorkerOrFunctionUrn {
    fn resource_type() -> &'static str {
        WORKER_TYPE_NAME
    }

    fn try_from_name(resource_name: &str) -> Result<Self, GolemUrnTransformError> {
        let has_function = if let Some((_, rest)) = resource_name.split_once('/') {
            rest.contains('/')
        } else {
            false
        };

        if has_function {
            Ok(WorkerOrFunctionUrn::Function(
                WorkerFunctionUrn::try_from_name(resource_name)?,
            ))
        } else {
            Ok(WorkerOrFunctionUrn::Worker(WorkerUrn::try_from_name(
                resource_name,
            )?))
        }
    }

    fn to_name(&self) -> String {
        match self {
            WorkerOrFunctionUrn::Worker(w) => w.to_name(),
            WorkerOrFunctionUrn::Function(f) => f.to_name(),
        }
    }
}

urn_from_into!(WorkerOrFunctionUrn);

/// Typed Golem URN for API definition
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ApiDefinitionUrn {
    pub id: String,
    pub version: String,
}

impl TypedGolemUrn for ApiDefinitionUrn {
    fn resource_type() -> &'static str {
        API_DEFINITION_TYPE_NAME
    }

    fn try_from_name(resource_name: &str) -> Result<Self, GolemUrnTransformError> {
        if let Some((id, version)) = resource_name.split_once('/') {
            let id = urldecode(id);
            let version = urldecode(version);

            Ok(Self { id, version })
        } else {
            Err(GolemUrnTransformError::invalid_name(
                Self::resource_type(),
                "Version expected".to_string(),
            ))
        }
    }

    fn to_name(&self) -> String {
        let id: String = urlencode(&self.id);

        format!("{id}/{}", urlencode(&self.version))
    }
}

urn_from_into!(ApiDefinitionUrn);

/// Typed Golem URN for API deployment
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct ApiDeploymentUrn {
    pub site: String,
}

impl TypedGolemUrn for ApiDeploymentUrn {
    fn resource_type() -> &'static str {
        API_DEPLOYMENT_TYPE_NAME
    }

    fn try_from_name(resource_name: &str) -> Result<Self, GolemUrnTransformError> {
        Ok(ApiDeploymentUrn {
            site: resource_name.to_string(),
        })
    }

    fn to_name(&self) -> String {
        self.site.to_string()
    }
}

urn_from_into!(ApiDeploymentUrn);

/// Any valid URN for a known Golem resource
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ResourceUrn {
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
    use test_r::test;

    use crate::model::{ComponentId, TargetWorkerId, WorkerId};
    use crate::uri::oss::urn::{
        ApiDefinitionUrn, ApiDeploymentUrn, ComponentOrVersionUrn, ComponentUrn,
        ComponentVersionUrn, ResourceUrn, WorkerFunctionUrn, WorkerOrFunctionUrn, WorkerUrn,
    };
    use crate::uri::GolemUrn;
    use std::str::FromStr;
    use uuid::Uuid;

    #[test]
    pub fn component_urn_to_urn() {
        let typed = ComponentUrn {
            id: ComponentId(Uuid::parse_str("679ae459-8700-41d9-920c-7e2887459c94").unwrap()),
        };

        let untyped: GolemUrn = typed.into();
        assert_eq!(
            untyped.to_string(),
            "urn:component:679ae459-8700-41d9-920c-7e2887459c94"
        );
    }

    #[test]
    pub fn component_urn_from_urn() {
        let untyped =
            GolemUrn::from_str("urn:component:679ae459-8700-41d9-920c-7e2887459c94").unwrap();
        let typed: ComponentUrn = untyped.try_into().unwrap();

        assert_eq!(
            typed.id.0.to_string(),
            "679ae459-8700-41d9-920c-7e2887459c94"
        );
    }

    #[test]
    pub fn component_urn_from_str() {
        let typed =
            ComponentUrn::from_str("urn:component:679ae459-8700-41d9-920c-7e2887459c94").unwrap();

        assert_eq!(
            typed.id.0.to_string(),
            "679ae459-8700-41d9-920c-7e2887459c94"
        );
    }

    #[test]
    pub fn component_version_urn_to_urn() {
        let typed = ComponentVersionUrn {
            id: ComponentId(Uuid::parse_str("679ae459-8700-41d9-920c-7e2887459c94").unwrap()),
            version: 7,
        };

        let untyped: GolemUrn = typed.into();
        assert_eq!(
            untyped.to_string(),
            "urn:component:679ae459-8700-41d9-920c-7e2887459c94/7"
        );
    }

    #[test]
    pub fn component_version_urn_from_urn() {
        let untyped =
            GolemUrn::from_str("urn:component:679ae459-8700-41d9-920c-7e2887459c94/9").unwrap();
        let typed: ComponentVersionUrn = untyped.try_into().unwrap();

        assert_eq!(
            typed.id.0.to_string(),
            "679ae459-8700-41d9-920c-7e2887459c94"
        );
        assert_eq!(typed.version, 9);
    }

    #[test]
    pub fn component_version_urn_from_str() {
        let typed =
            ComponentVersionUrn::from_str("urn:component:679ae459-8700-41d9-920c-7e2887459c94/9")
                .unwrap();

        assert_eq!(
            typed.id.0.to_string(),
            "679ae459-8700-41d9-920c-7e2887459c94"
        );
        assert_eq!(typed.version, 9);
    }

    #[test]
    pub fn component_or_version_urn_to_urn() {
        let typed = ComponentOrVersionUrn::Component(ComponentUrn {
            id: ComponentId(Uuid::parse_str("679ae459-8700-41d9-920c-7e2887459c94").unwrap()),
        });

        let untyped: GolemUrn = typed.into();
        assert_eq!(
            untyped.to_string(),
            "urn:component:679ae459-8700-41d9-920c-7e2887459c94"
        );
    }

    #[test]
    pub fn component_or_version_urn_from_urn() {
        let typed_version: ComponentOrVersionUrn =
            GolemUrn::from_str("urn:component:679ae459-8700-41d9-920c-7e2887459c94/9")
                .unwrap()
                .try_into()
                .unwrap();
        let typed_no_version: ComponentOrVersionUrn =
            GolemUrn::from_str("urn:component:679ae459-8700-41d9-920c-7e2887459c94")
                .unwrap()
                .try_into()
                .unwrap();

        assert_eq!(
            GolemUrn::from(typed_version).to_string(),
            "urn:component:679ae459-8700-41d9-920c-7e2887459c94/9"
        );
        assert_eq!(
            GolemUrn::from(typed_no_version).to_string(),
            "urn:component:679ae459-8700-41d9-920c-7e2887459c94"
        );
    }

    #[test]
    pub fn component_or_version_urn_from_str() {
        let typed_version =
            ComponentOrVersionUrn::from_str("urn:component:679ae459-8700-41d9-920c-7e2887459c94/9")
                .unwrap();
        let typed_no_version =
            ComponentOrVersionUrn::from_str("urn:component:679ae459-8700-41d9-920c-7e2887459c94")
                .unwrap();

        assert_eq!(
            typed_version.to_string(),
            "urn:component:679ae459-8700-41d9-920c-7e2887459c94/9"
        );
        assert_eq!(
            typed_no_version.to_string(),
            "urn:component:679ae459-8700-41d9-920c-7e2887459c94"
        );
    }

    #[test]
    pub fn worker_urn_to_urn() {
        let typed = WorkerUrn {
            id: TargetWorkerId {
                component_id: ComponentId(
                    Uuid::parse_str("679ae459-8700-41d9-920c-7e2887459c94").unwrap(),
                ),
                worker_name: Some("my:worker/1".to_string()),
            },
        };

        let untyped: GolemUrn = typed.into();
        assert_eq!(
            untyped.to_string(),
            "urn:worker:679ae459-8700-41d9-920c-7e2887459c94/my%3Aworker%2F1"
        );
    }

    #[test]
    pub fn worker_urn_to_urn_no_name() {
        let typed = WorkerUrn {
            id: TargetWorkerId {
                component_id: ComponentId(
                    Uuid::parse_str("679ae459-8700-41d9-920c-7e2887459c94").unwrap(),
                ),
                worker_name: None,
            },
        };

        let untyped: GolemUrn = typed.into();
        assert_eq!(
            untyped.to_string(),
            "urn:worker:679ae459-8700-41d9-920c-7e2887459c94"
        );
    }

    #[test]
    pub fn worker_urn_from_urn() {
        let untyped =
            GolemUrn::from_str("urn:worker:679ae459-8700-41d9-920c-7e2887459c94/my%3Aworker%2F1")
                .unwrap();
        let typed: WorkerUrn = untyped.try_into().unwrap();

        assert_eq!(
            typed.id.component_id.0.to_string(),
            "679ae459-8700-41d9-920c-7e2887459c94"
        );
        assert_eq!(typed.id.worker_name, Some("my:worker/1".to_string()));
    }

    #[test]
    pub fn worker_urn_from_urn_no_name() {
        let untyped =
            GolemUrn::from_str("urn:worker:679ae459-8700-41d9-920c-7e2887459c94").unwrap();
        let typed: WorkerUrn = untyped.try_into().unwrap();

        assert_eq!(
            typed.id.component_id.0.to_string(),
            "679ae459-8700-41d9-920c-7e2887459c94"
        );
        assert_eq!(typed.id.worker_name, None);
    }

    #[test]
    pub fn worker_urn_from_str() {
        let typed =
            WorkerUrn::from_str("urn:worker:679ae459-8700-41d9-920c-7e2887459c94/my%3Aworker%2F1")
                .unwrap();

        assert_eq!(
            typed.id.component_id.0.to_string(),
            "679ae459-8700-41d9-920c-7e2887459c94"
        );
        assert_eq!(typed.id.worker_name, Some("my:worker/1".to_string()));
    }

    #[test]
    pub fn worker_urn_from_str_no_name() {
        let typed = WorkerUrn::from_str("urn:worker:679ae459-8700-41d9-920c-7e2887459c94").unwrap();

        assert_eq!(
            typed.id.component_id.0.to_string(),
            "679ae459-8700-41d9-920c-7e2887459c94"
        );
        assert_eq!(typed.id.worker_name, None);
    }

    #[test]
    pub fn worker_function_urn_to_urn() {
        let typed = WorkerFunctionUrn {
            id: WorkerId {
                component_id: ComponentId(
                    Uuid::parse_str("679ae459-8700-41d9-920c-7e2887459c94").unwrap(),
                ),
                worker_name: "my:worker/1".to_string(),
            },
            function: "fn a".to_string(),
        };

        let untyped: GolemUrn = typed.into();
        assert_eq!(
            untyped.to_string(),
            "urn:worker:679ae459-8700-41d9-920c-7e2887459c94/my%3Aworker%2F1/fn+a"
        );
    }

    #[test]
    pub fn worker_function_urn_from_urn() {
        let untyped = GolemUrn::from_str(
            "urn:worker:679ae459-8700-41d9-920c-7e2887459c94/my%3Aworker%2F1/fn+a",
        )
        .unwrap();
        let typed: WorkerFunctionUrn = untyped.try_into().unwrap();

        assert_eq!(
            typed.id.component_id.0.to_string(),
            "679ae459-8700-41d9-920c-7e2887459c94"
        );
        assert_eq!(typed.id.worker_name, "my:worker/1");
        assert_eq!(typed.function, "fn a");
    }

    #[test]
    pub fn worker_function_urn_from_str() {
        let typed = WorkerFunctionUrn::from_str(
            "urn:worker:679ae459-8700-41d9-920c-7e2887459c94/my%3Aworker%2F1/fn+a",
        )
        .unwrap();

        assert_eq!(
            typed.id.component_id.0.to_string(),
            "679ae459-8700-41d9-920c-7e2887459c94"
        );
        assert_eq!(typed.id.worker_name, "my:worker/1");
        assert_eq!(typed.function, "fn a");
    }

    #[test]
    pub fn worker_or_function_urn_to_urn() {
        let typed_w = WorkerOrFunctionUrn::Worker(WorkerUrn {
            id: TargetWorkerId {
                component_id: ComponentId(
                    Uuid::parse_str("679ae459-8700-41d9-920c-7e2887459c94").unwrap(),
                ),
                worker_name: Some("my:worker/1".to_string()),
            },
        });
        let typed_f = WorkerOrFunctionUrn::Function(WorkerFunctionUrn {
            id: WorkerId {
                component_id: ComponentId(
                    Uuid::parse_str("679ae459-8700-41d9-920c-7e2887459c94").unwrap(),
                ),
                worker_name: "my:worker/1".to_string(),
            },
            function: "fn a".to_string(),
        });

        let untyped_w: GolemUrn = typed_w.into();
        let untyped_f: GolemUrn = typed_f.into();

        assert_eq!(
            untyped_w.to_string(),
            "urn:worker:679ae459-8700-41d9-920c-7e2887459c94/my%3Aworker%2F1"
        );
        assert_eq!(
            untyped_f.to_string(),
            "urn:worker:679ae459-8700-41d9-920c-7e2887459c94/my%3Aworker%2F1/fn+a"
        );
    }

    #[test]
    pub fn worker_or_function_urn_from_urn() {
        let untyped_w =
            GolemUrn::from_str("urn:worker:679ae459-8700-41d9-920c-7e2887459c94/my%3Aworker%2F1")
                .unwrap();
        let untyped_f = GolemUrn::from_str(
            "urn:worker:679ae459-8700-41d9-920c-7e2887459c94/my%3Aworker%2F1/fn+a",
        )
        .unwrap();

        let typed_w: WorkerOrFunctionUrn = untyped_w.try_into().unwrap();
        let typed_f: WorkerOrFunctionUrn = untyped_f.try_into().unwrap();

        assert_eq!(
            typed_w.to_string(),
            "urn:worker:679ae459-8700-41d9-920c-7e2887459c94/my%3Aworker%2F1"
        );
        assert_eq!(
            typed_f.to_string(),
            "urn:worker:679ae459-8700-41d9-920c-7e2887459c94/my%3Aworker%2F1/fn+a"
        );
    }

    #[test]
    pub fn worker_or_function_urn_from_str() {
        let typed_w = WorkerOrFunctionUrn::from_str(
            "urn:worker:679ae459-8700-41d9-920c-7e2887459c94/my%3Aworker%2F1",
        )
        .unwrap();
        let typed_f = WorkerOrFunctionUrn::from_str(
            "urn:worker:679ae459-8700-41d9-920c-7e2887459c94/my%3Aworker%2F1/fn+a",
        )
        .unwrap();

        assert_eq!(
            typed_w.to_string(),
            "urn:worker:679ae459-8700-41d9-920c-7e2887459c94/my%3Aworker%2F1"
        );
        assert_eq!(
            typed_f.to_string(),
            "urn:worker:679ae459-8700-41d9-920c-7e2887459c94/my%3Aworker%2F1/fn+a"
        );
    }

    #[test]
    pub fn api_definition_urn_to_urn() {
        let typed = ApiDefinitionUrn {
            id: "a:b.c".to_string(),
            version: "1.2.3".to_string(),
        };

        let untyped: GolemUrn = typed.into();
        assert_eq!(untyped.to_string(), "urn:api-definition:a%3Ab.c/1.2.3");
    }

    #[test]
    pub fn api_definition_urn_from_urn() {
        let untyped = GolemUrn::from_str("urn:api-definition:a%3Ab.c/1.2.3").unwrap();
        let typed: ApiDefinitionUrn = untyped.try_into().unwrap();

        assert_eq!(typed.id, "a:b.c");
        assert_eq!(typed.version, "1.2.3");
    }

    #[test]
    pub fn api_definition_urn_from_str() {
        let typed = ApiDefinitionUrn::from_str("urn:api-definition:a%3Ab.c/1.2.3").unwrap();

        assert_eq!(typed.id, "a:b.c");
        assert_eq!(typed.version, "1.2.3");
    }

    #[test]
    pub fn api_deployment_urn_to_urn() {
        let typed = ApiDeploymentUrn {
            site: "example.com".to_string(),
        };

        let untyped: GolemUrn = typed.into();
        assert_eq!(untyped.to_string(), "urn:api-deployment:example.com");
    }

    #[test]
    pub fn api_deployment_urn_from_urn() {
        let untyped = GolemUrn::from_str("urn:api-deployment:example.com").unwrap();
        let typed: ApiDeploymentUrn = untyped.try_into().unwrap();

        assert_eq!(typed.site, "example.com");
    }

    #[test]
    pub fn api_deployment_urn_from_str() {
        let typed = ApiDeploymentUrn::from_str("urn:api-deployment:example.com").unwrap();

        assert_eq!(typed.site, "example.com");
    }

    #[test]
    pub fn resource_urn_from_urn() {
        let untyped_cv =
            GolemUrn::from_str("urn:component:679ae459-8700-41d9-920c-7e2887459c94/7").unwrap();
        let untyped_ad = GolemUrn::from_str("urn:api-deployment:example.com").unwrap();
        let typed_cv: ResourceUrn = untyped_cv.try_into().unwrap();
        let typed_ad: ResourceUrn = untyped_ad.try_into().unwrap();

        assert_eq!(
            GolemUrn::from(typed_cv).to_string(),
            "urn:component:679ae459-8700-41d9-920c-7e2887459c94/7"
        );
        assert_eq!(
            GolemUrn::from(typed_ad).to_string(),
            "urn:api-deployment:example.com"
        );
    }

    #[test]
    pub fn resource_urn_from_str() {
        let typed_cv =
            ResourceUrn::from_str("urn:component:679ae459-8700-41d9-920c-7e2887459c94/7").unwrap();
        let typed_ad = ResourceUrn::from_str("urn:api-deployment:example.com").unwrap();

        assert_eq!(
            typed_cv.to_string(),
            "urn:component:679ae459-8700-41d9-920c-7e2887459c94/7"
        );
        assert_eq!(typed_ad.to_string(), "urn:api-deployment:example.com");
    }
}
