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

use crate::{ComponentDependencyKey, DynamicParsedFunctionName, Expr};
use crate::{FullyQualifiedResourceConstructor, InstanceType, VariableId};
use std::fmt::Display;

#[derive(Debug, Hash, PartialEq, Eq, Clone, Ord, PartialOrd)]
pub enum CallType {
    Function {
        component_info: Option<ComponentDependencyKey>,
        // as compilation progress the function call is expected to a instance_identifier
        // and will be always `Some`.
        instance_identifier: Option<InstanceIdentifier>,
        // TODO; a dynamic-parsed-function-name can be replaced by ParsedFunctionName
        // after the introduction of non-lazy resource constructor.
        function_name: DynamicParsedFunctionName,
    },
    VariantConstructor(String),
    EnumConstructor(String),
    InstanceCreation(InstanceCreationType),
}

#[derive(Debug, Hash, PartialEq, Eq, Clone, Ord, PartialOrd)]
pub struct InstanceIdentifier {
    // The variable-id of the worker instance or the resource instance,
    // `let x = instance(); x.foo()`.
    // module identifier here is variable-id x, with instance type being a worker
    pub variable_id: Option<VariableId>,
    pub instance_type: Box<InstanceType>,
}

#[derive(Debug, Hash, PartialEq, Eq, Clone, Ord, PartialOrd)]
pub enum InstanceCreationType {
    // A wit worker instance can be created without another module
    WitWorker {
        component_info: Option<ComponentDependencyKey>,
        worker_name: Option<Box<Expr>>,
    },
    // an instance type of the type wit-resource can only be part of
    // another instance (we call it module), which can be theoretically only be
    // a worker, but we don't restrict this in types, such that it will easily
    // handle nested wit resources
    WitResource {
        component_info: Option<ComponentDependencyKey>,
        // this module identifier during resource creation will be always a worker module, but we don't necessarily restrict
        // i.e, we do allow nested resource construction
        module: Option<InstanceIdentifier>,
        resource_name: FullyQualifiedResourceConstructor,
    },
}

impl InstanceCreationType {
    pub fn worker_name(&self) -> Option<Expr> {
        match self {
            InstanceCreationType::WitWorker { worker_name, .. } => worker_name.as_deref().cloned(),
            InstanceCreationType::WitResource { module, .. } => {
                let r = module.as_ref().and_then(|m| m.instance_type.worker_name());
                r.as_deref().cloned()
            }
        }
    }
}

impl CallType {
    pub fn function_name(&self) -> Option<DynamicParsedFunctionName> {
        match self {
            CallType::Function { function_name, .. } => Some(function_name.clone()),
            _ => None,
        }
    }
    pub fn worker_expr(&self) -> Option<&Expr> {
        match self {
            CallType::Function {
                instance_identifier: worker,
                ..
            } => {
                let module = worker.as_ref()?;
                let instance = &module.instance_type;
                instance.worker()
            }
            _ => None,
        }
    }

    pub fn worker_expr_mut(&mut self) -> Option<&mut Box<Expr>> {
        match self {
            CallType::Function {
                instance_identifier: module,
                ..
            } => {
                let module = module.as_mut()?;
                let instance = &mut *module.instance_type;
                instance.worker_mut()
            }
            _ => None,
        }
    }
    pub fn function_call(
        function: DynamicParsedFunctionName,
        component_info: Option<ComponentDependencyKey>,
    ) -> CallType {
        CallType::Function {
            instance_identifier: None,
            function_name: function,
            component_info,
        }
    }

    pub fn function_call_with_worker(
        module: InstanceIdentifier,
        function: DynamicParsedFunctionName,
        component_info: Option<ComponentDependencyKey>,
    ) -> CallType {
        CallType::Function {
            instance_identifier: Some(module),
            function_name: function,
            component_info,
        }
    }

    pub fn is_resource_method(&self) -> bool {
        match self {
            CallType::Function { function_name, .. } => function_name
                .to_parsed_function_name()
                .function
                .resource_method_name()
                .is_some(),
            _ => false,
        }
    }
}

impl Display for CallType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CallType::Function { function_name, .. } => write!(f, "{}", function_name),
            CallType::VariantConstructor(name) => write!(f, "{}", name),
            CallType::EnumConstructor(name) => write!(f, "{}", name),
            CallType::InstanceCreation(instance_creation_type) => match instance_creation_type {
                InstanceCreationType::WitWorker { .. } => {
                    write!(f, "instance")
                }
                InstanceCreationType::WitResource { resource_name, .. } => {
                    write!(f, "{}", resource_name.resource_name)
                }
            },
        }
    }
}

#[cfg(feature = "protobuf")]
mod protobuf {
    use crate::call_type::{CallType, InstanceCreationType};
    use crate::FullyQualifiedResourceConstructor;
    use crate::{ComponentDependencyKey, DynamicParsedFunctionName, Expr, ParsedFunctionName};
    use golem_api_grpc::proto::golem::rib::WorkerInstance;

    impl TryFrom<golem_api_grpc::proto::golem::rib::ComponentDependencyKey> for ComponentDependencyKey {
        type Error = String;

        fn try_from(
            value: golem_api_grpc::proto::golem::rib::ComponentDependencyKey,
        ) -> Result<Self, Self::Error> {
            let component_name = value.component_name;
            let component_id = value.value.ok_or("Missing component id")?;

            let root_package_name = value.root_package_name;

            let root_package_version = value.root_package_version;

            Ok(ComponentDependencyKey {
                component_name,
                component_id: component_id.into(),
                root_package_name,
                root_package_version,
            })
        }
    }

    impl From<ComponentDependencyKey> for golem_api_grpc::proto::golem::rib::ComponentDependencyKey {
        fn from(value: ComponentDependencyKey) -> Self {
            golem_api_grpc::proto::golem::rib::ComponentDependencyKey {
                component_name: value.component_name,
                value: Some(value.component_id.into()),
                root_package_name: value.root_package_name,
                root_package_version: value.root_package_version,
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::rib::InstanceCreationType> for InstanceCreationType {
        type Error = String;
        fn try_from(
            value: golem_api_grpc::proto::golem::rib::InstanceCreationType,
        ) -> Result<Self, Self::Error> {
            match value.kind.ok_or("Missing instance creation kind")? {
                golem_api_grpc::proto::golem::rib::instance_creation_type::Kind::Worker(
                    worker_instance,
                ) => {
                    let worker_name = worker_instance
                        .worker_name
                        .map(|w| Expr::try_from(*w))
                        .transpose()?
                        .map(Box::new);

                    Ok(InstanceCreationType::WitWorker {
                        component_info: None,
                        worker_name,
                    })
                }
                golem_api_grpc::proto::golem::rib::instance_creation_type::Kind::Resource(
                    resource_instance,
                ) => {
                    let resource_constructor_proto = resource_instance
                        .resource_name
                        .ok_or("Missing resource name")?;
                    let resource_name =
                        FullyQualifiedResourceConstructor::try_from(resource_constructor_proto)?;

                    let component_info = resource_instance
                        .component
                        .map(ComponentDependencyKey::try_from)
                        .transpose()?;

                    Ok(InstanceCreationType::WitResource {
                        component_info,
                        module: None,
                        resource_name,
                    })
                }
            }
        }
    }

    impl From<InstanceCreationType> for golem_api_grpc::proto::golem::rib::InstanceCreationType {
        fn from(value: InstanceCreationType) -> Self {
            match value {
                InstanceCreationType::WitWorker { component_info, .. } => {
                    golem_api_grpc::proto::golem::rib::InstanceCreationType {
                        kind: Some(golem_api_grpc::proto::golem::rib::instance_creation_type::Kind::Worker(Box::new(WorkerInstance {
                            component: component_info.map(golem_api_grpc::proto::golem::rib::ComponentDependencyKey::from),
                            worker_name: None
                        }))),
                    }
                }
                InstanceCreationType::WitResource { component_info, module: worker_name, resource_name } => {
                    golem_api_grpc::proto::golem::rib::InstanceCreationType {
                        kind: Some(golem_api_grpc::proto::golem::rib::instance_creation_type::Kind::Resource(Box::new(golem_api_grpc::proto::golem::rib::ResourceInstanceWithWorkerName {
                            component: component_info.map(golem_api_grpc::proto::golem::rib::ComponentDependencyKey::from),
                            worker_name: None,
                            resource_name: Some(golem_api_grpc::proto::golem::rib::FullyQualifiedResourceConstructor::from(resource_name)),
                        }))),
                    }
                }
            }
        }
    }

    impl TryFrom<golem_api_grpc::proto::golem::rib::CallType> for CallType {
        type Error = String;
        fn try_from(
            value: golem_api_grpc::proto::golem::rib::CallType,
        ) -> Result<Self, Self::Error> {
            let invocation = value.name.ok_or("Missing name of invocation")?;
            let worker = value
                .worker_name
                .map(|w| Expr::try_from(*w))
                .transpose()?
                .map(Box::new);
            match invocation {
                golem_api_grpc::proto::golem::rib::call_type::Name::Parsed(name) => {
                    Ok(CallType::Function {
                        component_info: None,
                        function_name: DynamicParsedFunctionName::try_from(name)?,
                        instance_identifier: None,
                    })
                }
                golem_api_grpc::proto::golem::rib::call_type::Name::VariantConstructor(name) => {
                    Ok(CallType::VariantConstructor(name))
                }
                golem_api_grpc::proto::golem::rib::call_type::Name::EnumConstructor(name) => {
                    Ok(CallType::EnumConstructor(name))
                }

                golem_api_grpc::proto::golem::rib::call_type::Name::InstanceCreation(
                    instance_creation,
                ) => {
                    let instance_creation = InstanceCreationType::try_from(*instance_creation)?;
                    Ok(CallType::InstanceCreation(instance_creation))
                }
            }
        }
    }

    impl From<CallType> for golem_api_grpc::proto::golem::rib::CallType {
        fn from(value: CallType) -> Self {
            match value {
                CallType::Function {
                    instance_identifier: module,
                    function_name,
                    ..
                } => golem_api_grpc::proto::golem::rib::CallType {
                    worker_name: None,
                    name: Some(golem_api_grpc::proto::golem::rib::call_type::Name::Parsed(
                        function_name.into(),
                    )),
                },
                CallType::VariantConstructor(name) => golem_api_grpc::proto::golem::rib::CallType {
                    worker_name: None,
                    name: Some(
                        golem_api_grpc::proto::golem::rib::call_type::Name::VariantConstructor(
                            name,
                        ),
                    ),
                },
                CallType::EnumConstructor(name) => golem_api_grpc::proto::golem::rib::CallType {
                    worker_name: None,
                    name: Some(
                        golem_api_grpc::proto::golem::rib::call_type::Name::EnumConstructor(name),
                    ),
                },
                CallType::InstanceCreation(instance_creation) => {
                    match instance_creation {
                        InstanceCreationType::WitWorker { worker_name , component_info} => {
                            golem_api_grpc::proto::golem::rib::CallType {
                                worker_name: worker_name.clone().map(|w| Box::new(golem_api_grpc::proto::golem::rib::Expr::from(*w))),
                                name:  Some(golem_api_grpc::proto::golem::rib::call_type::Name::InstanceCreation(
                                    Box::new(golem_api_grpc::proto::golem::rib::InstanceCreationType {
                                        kind: Some(golem_api_grpc::proto::golem::rib::instance_creation_type::Kind::Worker(Box::new(WorkerInstance {
                                            component: component_info.map(golem_api_grpc::proto::golem::rib::ComponentDependencyKey::from),
                                            worker_name: worker_name.map(|w| Box::new(golem_api_grpc::proto::golem::rib::Expr::from(*w))),
                                        }))),
                                    })
                                )),
                            }
                        }
                        InstanceCreationType::WitResource { module: worker_name, resource_name, component_info } => {
                            golem_api_grpc::proto::golem::rib::CallType {
                                worker_name: None,
                                name:  Some(golem_api_grpc::proto::golem::rib::call_type::Name::InstanceCreation(
                                    Box::new(golem_api_grpc::proto::golem::rib::InstanceCreationType {
                                        kind: Some(golem_api_grpc::proto::golem::rib::instance_creation_type::Kind::Resource(Box::new(golem_api_grpc::proto::golem::rib::ResourceInstanceWithWorkerName {
                                            component: component_info.map(golem_api_grpc::proto::golem::rib::ComponentDependencyKey::from),
                                            worker_name: None,
                                            resource_name: Some(golem_api_grpc::proto::golem::rib::FullyQualifiedResourceConstructor::from(resource_name)),
                                        }))),
                                    })
                                )),
                            }
                        }
                    }
                }
            }
        }
    }

    // InvocationName is a legacy structure to keep the backward compatibility.
    // InvocationName is corresponding to the new CallType and the difference here is,
    // InvocationName::Function will always hold a static function name and not a dynamic one
    // with Expr representing resource construction parameters
    impl TryFrom<golem_api_grpc::proto::golem::rib::InvocationName> for CallType {
        type Error = String;
        fn try_from(
            value: golem_api_grpc::proto::golem::rib::InvocationName,
        ) -> Result<Self, Self::Error> {
            let invocation = value.name.ok_or("Missing name of invocation")?;
            match invocation {
                golem_api_grpc::proto::golem::rib::invocation_name::Name::Parsed(name) => {
                    Ok(CallType::Function {
                        component_info: None,
                        instance_identifier: None,
                        function_name: DynamicParsedFunctionName::parse(
                            ParsedFunctionName::try_from(name)?.to_string(),
                        )?,
                    })
                }
                golem_api_grpc::proto::golem::rib::invocation_name::Name::VariantConstructor(
                    name,
                ) => Ok(CallType::VariantConstructor(name)),
                golem_api_grpc::proto::golem::rib::invocation_name::Name::EnumConstructor(name) => {
                    Ok(CallType::EnumConstructor(name))
                }
            }
        }
    }
}
