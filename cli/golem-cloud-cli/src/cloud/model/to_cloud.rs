use std::collections::HashMap;

pub trait ToCloud<T> {
    fn to_cloud(self) -> T;
}

impl<A: ToCloud<B>, B> ToCloud<Box<B>> for Box<A> {
    fn to_cloud(self) -> Box<B> {
        Box::new((*self).to_cloud())
    }
}

impl<A: ToCloud<B>, B> ToCloud<Option<B>> for Option<A> {
    fn to_cloud(self) -> Option<B> {
        self.map(|v| v.to_cloud())
    }
}

impl<A: ToCloud<B>, B> ToCloud<Vec<B>> for Vec<A> {
    fn to_cloud(self) -> Vec<B> {
        self.into_iter().map(|v| v.to_cloud()).collect()
    }
}

impl ToCloud<golem_cloud_client::model::ComponentType> for golem_client::model::ComponentType {
    fn to_cloud(self) -> golem_cloud_client::model::ComponentType {
        match self {
            golem_client::model::ComponentType::Durable => {
                golem_cloud_client::model::ComponentType::Durable
            }
            golem_client::model::ComponentType::Ephemeral => {
                golem_cloud_client::model::ComponentType::Ephemeral
            }
        }
    }
}

impl ToCloud<golem_cloud_client::model::ScanCursor> for golem_client::model::ScanCursor {
    fn to_cloud(self) -> golem_cloud_client::model::ScanCursor {
        golem_cloud_client::model::ScanCursor {
            cursor: self.cursor,
            layer: self.layer,
        }
    }
}

impl ToCloud<golem_cloud_client::model::InvokeParameters>
    for golem_client::model::InvokeParameters
{
    fn to_cloud(self) -> golem_cloud_client::model::InvokeParameters {
        golem_cloud_client::model::InvokeParameters {
            params: self.params,
        }
    }
}

impl ToCloud<golem_cloud_client::model::DynamicLinking> for golem_client::model::DynamicLinking {
    fn to_cloud(self) -> golem_cloud_client::model::DynamicLinking {
        golem_cloud_client::model::DynamicLinking {
            dynamic_linking: self
                .dynamic_linking
                .iter()
                .map(|(key, oss_instance)| {
                    (
                        key.clone(),
                        golem_cloud_client::model::DynamicLinkedInstance::WasmRpc(
                            golem_cloud_client::model::DynamicLinkedWasmRpc {
                                target_interface_name: match oss_instance {
                                    golem_client::model::DynamicLinkedInstance::WasmRpc(
                                        target_interface_name,
                                    ) => target_interface_name.clone().target_interface_name,
                                },
                            },
                        ),
                    )
                })
                .collect::<HashMap<_, _>>(),
        }
    }
}
