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

use crate::instance_type::FullyQualifiedResourceConstructor;
use crate::{DynamicParsedFunctionName, Expr};
use std::fmt::Display;

#[derive(Debug, Hash, PartialEq, Eq, Clone, Ord, PartialOrd)]
pub enum CallType {
    Function {
        worker: Option<Box<Expr>>,
        function_name: DynamicParsedFunctionName,
    },
    VariantConstructor(String),
    EnumConstructor(String),
    InstanceCreation(InstanceCreationType),
}

#[derive(Debug, Hash, PartialEq, Eq, Clone, Ord, PartialOrd)]
pub enum InstanceCreationType {
    Worker {
        worker_name: Option<Box<Expr>>,
    },
    Resource {
        worker_name: Option<Box<Expr>>,
        resource_name: FullyQualifiedResourceConstructor,
    },
}

impl InstanceCreationType {
    pub fn worker_name(&self) -> Option<&Expr> {
        match self {
            InstanceCreationType::Worker { worker_name, .. } => worker_name.as_deref(),
            InstanceCreationType::Resource { worker_name, .. } => worker_name.as_deref(),
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
            CallType::Function { worker, .. } => worker.as_deref(),
            _ => None,
        }
    }

    pub fn worker_expr_mut(&mut self) -> Option<&mut Box<Expr>> {
        match self {
            CallType::Function { worker, .. } => worker.as_mut(),
            _ => None,
        }
    }
    pub fn function_without_worker(function: DynamicParsedFunctionName) -> CallType {
        CallType::Function {
            worker: None,
            function_name: function,
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
                InstanceCreationType::Worker { .. } => {
                    write!(f, "instance")
                }
                InstanceCreationType::Resource { resource_name, .. } => {
                    write!(f, "{}", resource_name.resource_name)
                }
            },
        }
    }
}

#[cfg(feature = "protobuf")]
mod protobuf {
    use crate::call_type::{CallType, InstanceCreationType};
    use crate::instance_type::FullyQualifiedResourceConstructor;
    use crate::{DynamicParsedFunctionName, Expr, ParsedFunctionName};
    use golem_api_grpc::proto::golem::rib::WorkerInstance;

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

                    Ok(InstanceCreationType::Worker { worker_name })
                }
                golem_api_grpc::proto::golem::rib::instance_creation_type::Kind::Resource(
                    resource_instance,
                ) => {
                    let worker_name = resource_instance
                        .worker_name
                        .map(|w| Expr::try_from(*w))
                        .transpose()?
                        .map(Box::new);
                    let resource_constructor_proto = resource_instance
                        .resource_name
                        .ok_or("Missing resource name")?;
                    let resource_name =
                        FullyQualifiedResourceConstructor::try_from(resource_constructor_proto)?;

                    Ok(InstanceCreationType::Resource {
                        worker_name,
                        resource_name,
                    })
                }
            }
        }
    }

    impl From<InstanceCreationType> for golem_api_grpc::proto::golem::rib::InstanceCreationType {
        fn from(value: InstanceCreationType) -> Self {
            match value {
                InstanceCreationType::Worker { worker_name } => {
                    golem_api_grpc::proto::golem::rib::InstanceCreationType {
                        kind: Some(golem_api_grpc::proto::golem::rib::instance_creation_type::Kind::Worker(Box::new(WorkerInstance {
                            worker_name: worker_name.clone().map(|w| Box::new(golem_api_grpc::proto::golem::rib::Expr::from(*w))),
                        }))),
                    }
                }
                InstanceCreationType::Resource { worker_name, resource_name } => {
                    golem_api_grpc::proto::golem::rib::InstanceCreationType {
                        kind: Some(golem_api_grpc::proto::golem::rib::instance_creation_type::Kind::Resource(Box::new(golem_api_grpc::proto::golem::rib::ResourceInstanceWithWorkerName {
                            worker_name: worker_name.clone().map(|w| Box::new(golem_api_grpc::proto::golem::rib::Expr::from(*w))),
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
                        function_name: DynamicParsedFunctionName::try_from(name)?,
                        worker,
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
                    worker,
                    function_name,
                } => golem_api_grpc::proto::golem::rib::CallType {
                    worker_name: worker.map(|w| Box::new(golem_api_grpc::proto::golem::rib::Expr::from(*w))),
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
                        InstanceCreationType::Worker { worker_name } => {
                            golem_api_grpc::proto::golem::rib::CallType {
                                worker_name: worker_name.clone().map(|w| Box::new(golem_api_grpc::proto::golem::rib::Expr::from(*w))),
                                name:  Some(golem_api_grpc::proto::golem::rib::call_type::Name::InstanceCreation(
                                    Box::new(golem_api_grpc::proto::golem::rib::InstanceCreationType {
                                        kind: Some(golem_api_grpc::proto::golem::rib::instance_creation_type::Kind::Worker(Box::new(WorkerInstance {
                                            worker_name: worker_name.map(|w| Box::new(golem_api_grpc::proto::golem::rib::Expr::from(*w))),
                                        }))),
                                    })
                                )),
                            }
                        }
                        InstanceCreationType::Resource { worker_name, resource_name } => {
                            golem_api_grpc::proto::golem::rib::CallType {
                                worker_name: worker_name.clone().map(|w| Box::new(golem_api_grpc::proto::golem::rib::Expr::from(*w))),
                                name:  Some(golem_api_grpc::proto::golem::rib::call_type::Name::InstanceCreation(
                                    Box::new(golem_api_grpc::proto::golem::rib::InstanceCreationType {
                                        kind: Some(golem_api_grpc::proto::golem::rib::instance_creation_type::Kind::Resource(Box::new(golem_api_grpc::proto::golem::rib::ResourceInstanceWithWorkerName {
                                            worker_name: worker_name.map(|w| Box::new(golem_api_grpc::proto::golem::rib::Expr::from(*w))),
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
                        worker: None,
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
