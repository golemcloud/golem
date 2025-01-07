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

use crate::uri::oss::url::{
    ApiDefinitionUrl, ApiDeploymentUrl, ComponentOrVersionUrl, ComponentUrl, ComponentVersionUrl,
    ResourceUrl, WorkerFunctionUrl, WorkerOrFunctionUrl, WorkerUrl,
};
use crate::uri::oss::urn::{
    ApiDefinitionUrn, ApiDeploymentUrn, ComponentOrVersionUrn, ComponentUrn, ComponentVersionUrn,
    ResourceUrn, WorkerFunctionUrn, WorkerOrFunctionUrn, WorkerUrn,
};
use crate::uri::{GolemUri, GolemUriParseError, GolemUrlTransformError, GolemUrnTransformError};
use crate::uri_from_into;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::error::Error;
use std::fmt::{Display, Formatter};
use std::str::FromStr;

#[derive(Debug, Clone)]
pub enum GolemUriTransformError {
    URN(GolemUrnTransformError),
    URL(GolemUrlTransformError),
}

impl Error for GolemUriTransformError {}

impl Display for GolemUriTransformError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            GolemUriTransformError::URN(err) => {
                write!(f, "{err}")
            }
            GolemUriTransformError::URL(err) => {
                write!(f, "{err}")
            }
        }
    }
}

/// Typed Golem URI for component
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ComponentUri {
    URN(ComponentUrn),
    URL(ComponentUrl),
}

uri_from_into!(ComponentUri);

/// Typed Golem URI for component version
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ComponentVersionUri {
    URN(ComponentVersionUrn),
    URL(ComponentVersionUrl),
}

uri_from_into!(ComponentVersionUri);

/// Typed Golem URI for component or component version
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ComponentOrVersionUri {
    URN(ComponentOrVersionUrn),
    URL(ComponentOrVersionUrl),
}

uri_from_into!(ComponentOrVersionUri);

/// Typed Golem URI for worker
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum WorkerUri {
    URN(WorkerUrn),
    URL(WorkerUrl),
}

uri_from_into!(WorkerUri);

/// Typed Golem URI for worker function
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum WorkerFunctionUri {
    URN(WorkerFunctionUrn),
    URL(WorkerFunctionUrl),
}

uri_from_into!(WorkerFunctionUri);

/// Typed Golem URI for worker or worker function
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum WorkerOrFunctionUri {
    URN(WorkerOrFunctionUrn),
    URL(WorkerOrFunctionUrl),
}

uri_from_into!(WorkerOrFunctionUri);

/// Typed Golem URI for API definition
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ApiDefinitionUri {
    URN(ApiDefinitionUrn),
    URL(ApiDefinitionUrl),
}

uri_from_into!(ApiDefinitionUri);

/// Typed Golem URI for API definition
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ApiDeploymentUri {
    URN(ApiDeploymentUrn),
    URL(ApiDeploymentUrl),
}

uri_from_into!(ApiDeploymentUri);

/// Any valid URI for a known Golem resource
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ResourceUri {
    URN(ResourceUrn),
    URL(ResourceUrl),
}

uri_from_into!(ResourceUri);

#[cfg(test)]
mod tests {
    use test_r::test;

    use crate::model::{ComponentId, TargetWorkerId, WorkerId};
    use crate::uri::oss::uri::{
        ApiDefinitionUri, ApiDeploymentUri, ComponentOrVersionUri, ComponentUri,
        ComponentVersionUri, ResourceUri, WorkerFunctionUri, WorkerOrFunctionUri, WorkerUri,
    };
    use crate::uri::oss::url::{
        ApiDefinitionUrl, ApiDeploymentUrl, ComponentOrVersionUrl, ComponentUrl,
        ComponentVersionUrl, WorkerFunctionUrl, WorkerOrFunctionUrl, WorkerUrl,
    };
    use crate::uri::oss::urn::{
        ApiDefinitionUrn, ApiDeploymentUrn, ComponentOrVersionUrn, ComponentUrn,
        ComponentVersionUrn, WorkerFunctionUrn, WorkerOrFunctionUrn, WorkerUrn,
    };
    use crate::uri::GolemUri;
    use std::str::FromStr;
    use uuid::Uuid;

    #[test]
    pub fn component_uri_to_uri() {
        let typed_url = ComponentUri::URL(ComponentUrl {
            name: "some  name".to_string(),
        });
        let typed_urn = ComponentUri::URN(ComponentUrn {
            id: ComponentId(Uuid::parse_str("679ae459-8700-41d9-920c-7e2887459c94").unwrap()),
        });

        let untyped_url: GolemUri = typed_url.into();
        let untyped_urn: GolemUri = typed_urn.into();

        assert_eq!(untyped_url.to_string(), "component:///some++name");
        assert_eq!(
            untyped_urn.to_string(),
            "urn:component:679ae459-8700-41d9-920c-7e2887459c94"
        );
    }

    #[test]
    pub fn component_uri_from_uri() {
        let untyped_url = GolemUri::from_str("component:///some++name").unwrap();
        let untyped_urn =
            GolemUri::from_str("urn:component:679ae459-8700-41d9-920c-7e2887459c94").unwrap();
        let typed_url: ComponentUri = untyped_url.try_into().unwrap();
        let typed_urn: ComponentUri = untyped_urn.try_into().unwrap();
        let ComponentUri::URL(typed_url) = typed_url else {
            panic!()
        };
        let ComponentUri::URN(typed_urn) = typed_urn else {
            panic!()
        };

        assert_eq!(typed_url.name, "some  name");
        assert_eq!(
            typed_urn.id.0.to_string(),
            "679ae459-8700-41d9-920c-7e2887459c94"
        );
    }

    #[test]
    pub fn component_uri_from_str() {
        let typed_url = ComponentUri::from_str("component:///some++name").unwrap();
        let typed_urn =
            ComponentUri::from_str("urn:component:679ae459-8700-41d9-920c-7e2887459c94").unwrap();
        let ComponentUri::URL(typed_url) = typed_url else {
            panic!()
        };
        let ComponentUri::URN(typed_urn) = typed_urn else {
            panic!()
        };

        assert_eq!(typed_url.name, "some  name");
        assert_eq!(
            typed_urn.id.0.to_string(),
            "679ae459-8700-41d9-920c-7e2887459c94"
        );
    }

    #[test]
    pub fn component_version_uri_to_uri() {
        let typed_url = ComponentVersionUri::URL(ComponentVersionUrl {
            name: "some  name".to_string(),
            version: 13,
        });
        let typed_urn = ComponentVersionUri::URN(ComponentVersionUrn {
            id: ComponentId(Uuid::parse_str("679ae459-8700-41d9-920c-7e2887459c94").unwrap()),
            version: 15,
        });

        let untyped_url: GolemUri = typed_url.into();
        let untyped_urn: GolemUri = typed_urn.into();
        assert_eq!(untyped_url.to_string(), "component:///some++name/13");
        assert_eq!(
            untyped_urn.to_string(),
            "urn:component:679ae459-8700-41d9-920c-7e2887459c94/15"
        );
    }

    #[test]
    pub fn component_version_uri_from_uri() {
        let untyped_url = GolemUri::from_str("component:///some++name/13").unwrap();
        let untyped_urn =
            GolemUri::from_str("urn:component:679ae459-8700-41d9-920c-7e2887459c94/15").unwrap();
        let typed_url: ComponentVersionUri = untyped_url.try_into().unwrap();
        let typed_urn: ComponentVersionUri = untyped_urn.try_into().unwrap();
        let ComponentVersionUri::URL(typed_url) = typed_url else {
            panic!()
        };
        let ComponentVersionUri::URN(typed_urn) = typed_urn else {
            panic!()
        };

        assert_eq!(typed_url.name, "some  name");
        assert_eq!(typed_url.version, 13);
        assert_eq!(
            typed_urn.id.0.to_string(),
            "679ae459-8700-41d9-920c-7e2887459c94"
        );
        assert_eq!(typed_urn.version, 15);
    }

    #[test]
    pub fn component_version_uri_from_str() {
        let typed_url = ComponentVersionUri::from_str("component:///some++name/13").unwrap();
        let typed_urn =
            ComponentVersionUri::from_str("urn:component:679ae459-8700-41d9-920c-7e2887459c94/15")
                .unwrap();
        let ComponentVersionUri::URL(typed_url) = typed_url else {
            panic!()
        };
        let ComponentVersionUri::URN(typed_urn) = typed_urn else {
            panic!()
        };

        assert_eq!(typed_url.name, "some  name");
        assert_eq!(typed_url.version, 13);
        assert_eq!(
            typed_urn.id.0.to_string(),
            "679ae459-8700-41d9-920c-7e2887459c94"
        );
        assert_eq!(typed_urn.version, 15);
    }

    #[test]
    pub fn component_or_version_uri_to_uri() {
        let typed_url =
            ComponentOrVersionUri::URL(ComponentOrVersionUrl::Version(ComponentVersionUrl {
                name: "some  name".to_string(),
                version: 13,
            }));
        let typed_urn =
            ComponentOrVersionUri::URN(ComponentOrVersionUrn::Component(ComponentUrn {
                id: ComponentId(Uuid::parse_str("679ae459-8700-41d9-920c-7e2887459c94").unwrap()),
            }));

        let untyped_url: GolemUri = typed_url.into();
        let untyped_urn: GolemUri = typed_urn.into();
        assert_eq!(untyped_url.to_string(), "component:///some++name/13");
        assert_eq!(
            untyped_urn.to_string(),
            "urn:component:679ae459-8700-41d9-920c-7e2887459c94"
        );
    }

    #[test]
    pub fn component_or_version_uri_from_uri() {
        let untyped_url = GolemUri::from_str("component:///some++name/13").unwrap();
        let untyped_urn =
            GolemUri::from_str("urn:component:679ae459-8700-41d9-920c-7e2887459c94").unwrap();
        let typed_url: ComponentOrVersionUri = untyped_url.try_into().unwrap();
        let typed_urn: ComponentOrVersionUri = untyped_urn.try_into().unwrap();

        assert_eq!(typed_url.to_string(), "component:///some++name/13");
        assert_eq!(
            typed_urn.to_string(),
            "urn:component:679ae459-8700-41d9-920c-7e2887459c94"
        );
    }

    #[test]
    pub fn component_or_version_uri_from_str() {
        let typed_url = ComponentOrVersionUri::from_str("component:///some++name/13").unwrap();
        let typed_urn =
            ComponentOrVersionUri::from_str("urn:component:679ae459-8700-41d9-920c-7e2887459c94")
                .unwrap();

        assert_eq!(typed_url.to_string(), "component:///some++name/13");
        assert_eq!(
            typed_urn.to_string(),
            "urn:component:679ae459-8700-41d9-920c-7e2887459c94"
        );
    }

    #[test]
    pub fn worker_uri_to_uri() {
        let typed_url = WorkerUri::URL(WorkerUrl {
            component_name: "my comp".to_string(),
            worker_name: Some("my worker".to_string()),
        });
        let typed_urn = WorkerUri::URN(WorkerUrn {
            id: TargetWorkerId {
                component_id: ComponentId(
                    Uuid::parse_str("679ae459-8700-41d9-920c-7e2887459c94").unwrap(),
                ),
                worker_name: Some("my worker".to_string()),
            },
        });

        let untyped_url: GolemUri = typed_url.into();
        let untyped_urn: GolemUri = typed_urn.into();
        assert_eq!(untyped_url.to_string(), "worker:///my+comp/my+worker");
        assert_eq!(
            untyped_urn.to_string(),
            "urn:worker:679ae459-8700-41d9-920c-7e2887459c94/my+worker"
        );
    }

    #[test]
    pub fn worker_uri_to_uri_no_name() {
        let typed_url = WorkerUri::URL(WorkerUrl {
            component_name: "my comp".to_string(),
            worker_name: None,
        });
        let typed_urn = WorkerUri::URN(WorkerUrn {
            id: TargetWorkerId {
                component_id: ComponentId(
                    Uuid::parse_str("679ae459-8700-41d9-920c-7e2887459c94").unwrap(),
                ),
                worker_name: None,
            },
        });

        let untyped_url: GolemUri = typed_url.into();
        let untyped_urn: GolemUri = typed_urn.into();
        assert_eq!(untyped_url.to_string(), "worker:///my+comp");
        assert_eq!(
            untyped_urn.to_string(),
            "urn:worker:679ae459-8700-41d9-920c-7e2887459c94"
        );
    }

    #[test]
    pub fn worker_uri_from_uri() {
        let untyped_url = GolemUri::from_str("worker:///my+comp/my+worker").unwrap();
        let untyped_urn =
            GolemUri::from_str("urn:worker:679ae459-8700-41d9-920c-7e2887459c94/my+worker")
                .unwrap();
        let typed_url: WorkerUri = untyped_url.try_into().unwrap();
        let typed_urn: WorkerUri = untyped_urn.try_into().unwrap();
        let WorkerUri::URL(typed_url) = typed_url else {
            panic!()
        };
        let WorkerUri::URN(typed_urn) = typed_urn else {
            panic!()
        };

        assert_eq!(typed_url.component_name, "my comp");
        assert_eq!(typed_url.worker_name, Some("my worker".to_string()));
        assert_eq!(
            typed_urn.id.component_id.0.to_string(),
            "679ae459-8700-41d9-920c-7e2887459c94"
        );
        assert_eq!(typed_urn.id.worker_name, Some("my worker".to_string()));
    }

    #[test]
    pub fn worker_uri_from_uri_no_name() {
        let untyped_url = GolemUri::from_str("worker:///my+comp").unwrap();
        let untyped_urn =
            GolemUri::from_str("urn:worker:679ae459-8700-41d9-920c-7e2887459c94").unwrap();
        let typed_url: WorkerUri = untyped_url.try_into().unwrap();
        let typed_urn: WorkerUri = untyped_urn.try_into().unwrap();
        let WorkerUri::URL(typed_url) = typed_url else {
            panic!()
        };
        let WorkerUri::URN(typed_urn) = typed_urn else {
            panic!()
        };

        assert_eq!(typed_url.component_name, "my comp");
        assert_eq!(typed_url.worker_name, None);
        assert_eq!(
            typed_urn.id.component_id.0.to_string(),
            "679ae459-8700-41d9-920c-7e2887459c94"
        );
        assert_eq!(typed_urn.id.worker_name, None);
    }

    #[test]
    pub fn worker_uri_from_str() {
        let typed_url = WorkerUri::from_str("worker:///my+comp/my+worker").unwrap();
        let typed_urn =
            WorkerUri::from_str("urn:worker:679ae459-8700-41d9-920c-7e2887459c94/my+worker")
                .unwrap();
        let WorkerUri::URL(typed_url) = typed_url else {
            panic!()
        };
        let WorkerUri::URN(typed_urn) = typed_urn else {
            panic!()
        };

        assert_eq!(typed_url.component_name, "my comp");
        assert_eq!(typed_url.worker_name, Some("my worker".to_string()));
        assert_eq!(
            typed_urn.id.component_id.0.to_string(),
            "679ae459-8700-41d9-920c-7e2887459c94"
        );
        assert_eq!(typed_urn.id.worker_name, Some("my worker".to_string()));
    }

    #[test]
    pub fn worker_uri_from_str_no_name() {
        let typed_url = WorkerUri::from_str("worker:///my+comp").unwrap();
        let typed_urn =
            WorkerUri::from_str("urn:worker:679ae459-8700-41d9-920c-7e2887459c94").unwrap();
        let WorkerUri::URL(typed_url) = typed_url else {
            panic!()
        };
        let WorkerUri::URN(typed_urn) = typed_urn else {
            panic!()
        };

        assert_eq!(typed_url.component_name, "my comp");
        assert_eq!(typed_url.worker_name, None);
        assert_eq!(
            typed_urn.id.component_id.0.to_string(),
            "679ae459-8700-41d9-920c-7e2887459c94"
        );
        assert_eq!(typed_urn.id.worker_name, None);
    }

    #[test]
    pub fn worker_function_uri_to_uri() {
        let typed_url = WorkerFunctionUri::URL(WorkerFunctionUrl {
            component_name: "my comp".to_string(),
            worker_name: "my worker".to_string(),
            function: "fn a".to_string(),
        });
        let typed_urn = WorkerFunctionUri::URN(WorkerFunctionUrn {
            id: WorkerId {
                component_id: ComponentId(
                    Uuid::parse_str("679ae459-8700-41d9-920c-7e2887459c94").unwrap(),
                ),
                worker_name: "my worker".to_string(),
            },
            function: "fn a".to_string(),
        });

        let untyped_url: GolemUri = typed_url.into();
        let untyped_urn: GolemUri = typed_urn.into();
        assert_eq!(untyped_url.to_string(), "worker:///my+comp/my+worker/fn+a");
        assert_eq!(
            untyped_urn.to_string(),
            "urn:worker:679ae459-8700-41d9-920c-7e2887459c94/my+worker/fn+a"
        );
    }

    #[test]
    pub fn worker_function_uri_from_uri() {
        let untyped_url = GolemUri::from_str("worker:///my+comp/my+worker/fn+a").unwrap();
        let untyped_urn =
            GolemUri::from_str("urn:worker:679ae459-8700-41d9-920c-7e2887459c94/my+worker/fn+a")
                .unwrap();
        let typed_url: WorkerFunctionUri = untyped_url.try_into().unwrap();
        let typed_urn: WorkerFunctionUri = untyped_urn.try_into().unwrap();
        let WorkerFunctionUri::URL(typed_url) = typed_url else {
            panic!()
        };
        let WorkerFunctionUri::URN(typed_urn) = typed_urn else {
            panic!()
        };

        assert_eq!(typed_url.component_name, "my comp");
        assert_eq!(typed_url.worker_name, "my worker");
        assert_eq!(typed_url.function, "fn a");
        assert_eq!(
            typed_urn.id.component_id.0.to_string(),
            "679ae459-8700-41d9-920c-7e2887459c94"
        );
        assert_eq!(typed_urn.id.worker_name, "my worker");
        assert_eq!(typed_urn.function, "fn a");
    }

    #[test]
    pub fn worker_function_uri_from_str() {
        let typed_url = WorkerFunctionUri::from_str("worker:///my+comp/my+worker/fn+a").unwrap();
        let typed_urn = WorkerFunctionUri::from_str(
            "urn:worker:679ae459-8700-41d9-920c-7e2887459c94/my+worker/fn+a",
        )
        .unwrap();
        let WorkerFunctionUri::URL(typed_url) = typed_url else {
            panic!()
        };
        let WorkerFunctionUri::URN(typed_urn) = typed_urn else {
            panic!()
        };

        assert_eq!(typed_url.component_name, "my comp");
        assert_eq!(typed_url.worker_name, "my worker");
        assert_eq!(typed_url.function, "fn a");
        assert_eq!(
            typed_urn.id.component_id.0.to_string(),
            "679ae459-8700-41d9-920c-7e2887459c94"
        );
        assert_eq!(typed_urn.id.worker_name, "my worker");
        assert_eq!(typed_urn.function, "fn a");
    }

    #[test]
    pub fn worker_or_function_uri_to_uri() {
        let typed_url =
            WorkerOrFunctionUri::URL(WorkerOrFunctionUrl::Function(WorkerFunctionUrl {
                component_name: "my comp".to_string(),
                worker_name: "my worker".to_string(),
                function: "fn a".to_string(),
            }));
        let typed_urn = WorkerOrFunctionUri::URN(WorkerOrFunctionUrn::Worker(WorkerUrn {
            id: TargetWorkerId {
                component_id: ComponentId(
                    Uuid::parse_str("679ae459-8700-41d9-920c-7e2887459c94").unwrap(),
                ),
                worker_name: Some("my worker".to_string()),
            },
        }));

        let untyped_url: GolemUri = typed_url.into();
        let untyped_urn: GolemUri = typed_urn.into();
        assert_eq!(untyped_url.to_string(), "worker:///my+comp/my+worker/fn+a");
        assert_eq!(
            untyped_urn.to_string(),
            "urn:worker:679ae459-8700-41d9-920c-7e2887459c94/my+worker"
        );
    }

    #[test]
    pub fn worker_or_function_uri_from_uri() {
        let untyped_url = GolemUri::from_str("worker:///my+comp/my+worker/fn+a").unwrap();
        let untyped_urn =
            GolemUri::from_str("urn:worker:679ae459-8700-41d9-920c-7e2887459c94/my+worker")
                .unwrap();
        let typed_url: WorkerOrFunctionUri = untyped_url.try_into().unwrap();
        let typed_urn: WorkerOrFunctionUri = untyped_urn.try_into().unwrap();

        assert_eq!(typed_url.to_string(), "worker:///my+comp/my+worker/fn+a");
        assert_eq!(
            typed_urn.to_string(),
            "urn:worker:679ae459-8700-41d9-920c-7e2887459c94/my+worker"
        );
    }

    #[test]
    pub fn worker_or_function_uri_from_str() {
        let typed_url = WorkerOrFunctionUri::from_str("worker:///my+comp/my+worker/fn+a").unwrap();
        let typed_urn = WorkerOrFunctionUri::from_str(
            "urn:worker:679ae459-8700-41d9-920c-7e2887459c94/my+worker",
        )
        .unwrap();

        assert_eq!(typed_url.to_string(), "worker:///my+comp/my+worker/fn+a");
        assert_eq!(
            typed_urn.to_string(),
            "urn:worker:679ae459-8700-41d9-920c-7e2887459c94/my+worker"
        );
    }

    #[test]
    pub fn api_definition_uri_to_uri() {
        let typed_url = ApiDefinitionUri::URL(ApiDefinitionUrl {
            name: "my def".to_string(),
            version: "1.2.3".to_string(),
        });
        let typed_urn = ApiDefinitionUri::URN(ApiDefinitionUrn {
            id: "my def".to_string(),
            version: "1.2.3".to_string(),
        });

        let untyped_url: GolemUri = typed_url.into();
        let untyped_urn: GolemUri = typed_urn.into();
        assert_eq!(untyped_url.to_string(), "api-definition:///my+def/1.2.3");
        assert_eq!(untyped_urn.to_string(), "urn:api-definition:my+def/1.2.3");
    }

    #[test]
    pub fn api_definition_uri_from_uri() {
        let untyped_url = GolemUri::from_str("api-definition:///my+def/1.2.3").unwrap();
        let untyped_urn = GolemUri::from_str("urn:api-definition:my+def/1.2.3").unwrap();
        let typed_url: ApiDefinitionUri = untyped_url.try_into().unwrap();
        let typed_urn: ApiDefinitionUri = untyped_urn.try_into().unwrap();
        let ApiDefinitionUri::URL(typed_url) = typed_url else {
            panic!()
        };
        let ApiDefinitionUri::URN(typed_urn) = typed_urn else {
            panic!()
        };

        assert_eq!(typed_url.name, "my def");
        assert_eq!(typed_url.version, "1.2.3");
        assert_eq!(typed_urn.id, "my def");
        assert_eq!(typed_urn.version, "1.2.3");
    }

    #[test]
    pub fn api_definition_uri_from_str() {
        let typed_url = ApiDefinitionUri::from_str("api-definition:///my+def/1.2.3").unwrap();
        let typed_urn = ApiDefinitionUri::from_str("urn:api-definition:my+def/1.2.3").unwrap();
        let ApiDefinitionUri::URL(typed_url) = typed_url else {
            panic!()
        };
        let ApiDefinitionUri::URN(typed_urn) = typed_urn else {
            panic!()
        };

        assert_eq!(typed_url.name, "my def");
        assert_eq!(typed_url.version, "1.2.3");
        assert_eq!(typed_urn.id, "my def");
        assert_eq!(typed_urn.version, "1.2.3");
    }

    #[test]
    pub fn api_deployment_uri_to_uri() {
        let typed_url = ApiDeploymentUri::URL(ApiDeploymentUrl {
            site: "example.com".to_string(),
        });
        let typed_urn = ApiDeploymentUri::URN(ApiDeploymentUrn {
            site: "example.com".to_string(),
        });

        let untyped_url: GolemUri = typed_url.into();
        let untyped_urn: GolemUri = typed_urn.into();
        assert_eq!(untyped_url.to_string(), "api-deployment:///example.com");
        assert_eq!(untyped_urn.to_string(), "urn:api-deployment:example.com");
    }

    #[test]
    pub fn api_deployment_uri_from_uri() {
        let untyped_url = GolemUri::from_str("api-deployment:///example.com").unwrap();
        let untyped_urn = GolemUri::from_str("urn:api-deployment:example.com").unwrap();
        let typed_url: ApiDeploymentUri = untyped_url.try_into().unwrap();
        let typed_urn: ApiDeploymentUri = untyped_urn.try_into().unwrap();
        let ApiDeploymentUri::URL(typed_url) = typed_url else {
            panic!()
        };
        let ApiDeploymentUri::URN(typed_urn) = typed_urn else {
            panic!()
        };

        assert_eq!(typed_url.site, "example.com");
        assert_eq!(typed_urn.site, "example.com");
    }

    #[test]
    pub fn api_deployment_uri_from_str() {
        let typed_url = ApiDeploymentUri::from_str("api-deployment:///example.com").unwrap();
        let typed_urn = ApiDeploymentUri::from_str("urn:api-deployment:example.com").unwrap();
        let ApiDeploymentUri::URL(typed_url) = typed_url else {
            panic!()
        };
        let ApiDeploymentUri::URN(typed_urn) = typed_urn else {
            panic!()
        };

        assert_eq!(typed_url.site, "example.com");
        assert_eq!(typed_urn.site, "example.com");
    }

    #[test]
    pub fn resource_uri_from_uri() {
        let untyped_cv = GolemUri::from_str("component:///comp_name/11").unwrap();
        let untyped_ad = GolemUri::from_str("urn:api-deployment:example.com").unwrap();
        let typed_cv: ResourceUri = untyped_cv.try_into().unwrap();
        let typed_ad: ResourceUri = untyped_ad.try_into().unwrap();

        assert_eq!(
            GolemUri::from(typed_cv).to_string(),
            "component:///comp_name/11"
        );
        assert_eq!(
            GolemUri::from(typed_ad).to_string(),
            "urn:api-deployment:example.com"
        );
    }

    #[test]
    pub fn resource_uri_from_str() {
        let typed_cv = ResourceUri::from_str("component:///comp_name/11").unwrap();
        let typed_ad = ResourceUri::from_str("urn:api-deployment:example.com").unwrap();

        assert_eq!(typed_cv.to_string(), "component:///comp_name/11");
        assert_eq!(typed_ad.to_string(), "urn:api-deployment:example.com");
    }
}
