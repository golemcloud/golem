pub mod proto {
    tonic::include_proto!("mod");

    // tonic::include_proto!("../golem-api-grpc/proto/golem");
    // include!(concat!(env!("OUT_DIR"), concat!("/", "", ".rs")));

    pub const FILE_DESCRIPTOR_SET: &[u8] = tonic::include_file_descriptor_set!("services");

    #[cfg(test)]
    mod tests {
        use std::collections::HashSet;

        use crate::proto::golem::cloud::projectpolicy::ProjectAction;
        use crate::proto::golem::cloud::project::{
            get_project_actions_response, GetProjectActionsResponse,
            GetProjectActionsSuccessResponse,
        };

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
