pub mod cache;
pub mod config;
pub mod metrics;
pub mod model;
pub mod newtype;
pub mod redis;
pub mod retries;
pub mod serialization;

pub mod proto {
    use uuid::Uuid;
    tonic::include_proto!("mod");

    pub const FILE_DESCRIPTOR_SET: &[u8] = tonic::include_file_descriptor_set!("services");

    impl From<Uuid> for golem::Uuid {
        fn from(value: Uuid) -> Self {
            let (high_bits, low_bits) = value.as_u64_pair();
            golem::Uuid {
                high_bits,
                low_bits,
            }
        }
    }

    impl From<golem::Uuid> for Uuid {
        fn from(value: golem::Uuid) -> Self {
            let high_bits = value.high_bits;
            let low_bits = value.low_bits;
            Uuid::from_u64_pair(high_bits, low_bits)
        }
    }

    #[cfg(test)]
    mod tests {
        use std::collections::HashSet;
        use std::str::FromStr;

        use crate::proto::golem;
        use crate::proto::golem::cloudservices::projectservice::{
            get_project_actions_response, GetProjectActionsResponse,
            GetProjectActionsSuccessResponse,
        };
        use crate::proto::golem::ProjectAction;

        #[test]
        fn test_uuid() {
            let project_id = uuid::Uuid::from_str("040eeaee-08fa-4273-83ea-bc26e10574c1").unwrap();
            let token = uuid::Uuid::from_str("5816ed13-4d6e-40d0-8391-f0eb75378476").unwrap();

            let project_id_proto: golem::Uuid = project_id.into();
            let token_proto: golem::Uuid = token.into();

            println!("project_id_proto: {:?}", project_id_proto);
            println!("token_proto: {:?}", token_proto);
        }

        #[test]
        fn test_project_action_conversion() {
            let grpc_response = GetProjectActionsResponse {
                result: Some(get_project_actions_response::Result::Success(
                    GetProjectActionsSuccessResponse {
                        data: vec![
                            ProjectAction::ViewWorker as i32,
                            ProjectAction::DeleteWorker as i32,
                            ProjectAction::ViewTemplate as i32,
                            ProjectAction::ViewProjectGrants as i32,
                            ProjectAction::ViewApiDefinition as i32,
                            ProjectAction::UpdateWorker as i32,
                            ProjectAction::UpdateApiDefinition as i32,
                            ProjectAction::CreateTemplate as i32,
                            ProjectAction::CreateProjectGrants as i32,
                            ProjectAction::UpdateTemplate as i32,
                            ProjectAction::DeleteTemplate as i32,
                            ProjectAction::CreateWorker as i32,
                            ProjectAction::DeleteProjectGrants as i32,
                            ProjectAction::DeleteApiDefinition as i32,
                            ProjectAction::CreateApiDefinition as i32,
                        ],
                    },
                )),
            };

            let actions = match grpc_response.result {
                None => panic!("Empty response"),
                Some(get_project_actions_response::Result::Success(response)) => {
                    let actions = response
                        .data
                        .iter()
                        .map(|n| {
                            ProjectAction::try_from(*n)
                                .map_err(|err| format!("Invalid action ({err})"))
                        })
                        .collect::<Result<HashSet<ProjectAction>, String>>();
                    actions
                }
                Some(get_project_actions_response::Result::Error(error)) => {
                    panic!("{error:?}")
                }
            };

            assert_eq!(
                actions,
                Ok(vec![
                    ProjectAction::ViewWorker,
                    ProjectAction::DeleteWorker,
                    ProjectAction::ViewTemplate,
                    ProjectAction::ViewProjectGrants,
                    ProjectAction::ViewApiDefinition,
                    ProjectAction::UpdateWorker,
                    ProjectAction::UpdateApiDefinition,
                    ProjectAction::CreateTemplate,
                    ProjectAction::CreateProjectGrants,
                    ProjectAction::UpdateTemplate,
                    ProjectAction::DeleteTemplate,
                    ProjectAction::CreateWorker,
                    ProjectAction::DeleteProjectGrants,
                    ProjectAction::DeleteApiDefinition,
                    ProjectAction::CreateApiDefinition,
                ]
                .into_iter()
                .collect::<HashSet<ProjectAction>>())
            );
        }
    }
}
