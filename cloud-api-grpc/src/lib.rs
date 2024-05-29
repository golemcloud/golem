pub mod proto {
    tonic::include_proto!("mod");

    pub const FILE_DESCRIPTOR_SET: &[u8] = tonic::include_file_descriptor_set!("services");

    #[cfg(test)]
    mod tests {
        use crate::proto::golem::cloud::projectpolicy::{ProjectAction, ProjectActions};
        use std::collections::HashSet;

        #[test]
        fn test_project_action_conversion() {
            let response = ProjectActions {
                actions: vec![
                    ProjectAction::ViewWorker as i32,
                    ProjectAction::DeleteWorker as i32,
                    ProjectAction::ViewComponent as i32,
                    ProjectAction::ViewProjectGrants as i32,
                    ProjectAction::ViewApiDefinition as i32,
                    ProjectAction::UpdateWorker as i32,
                    ProjectAction::UpdateApiDefinition as i32,
                    ProjectAction::CreateComponent as i32,
                    ProjectAction::CreateProjectGrants as i32,
                    ProjectAction::UpdateComponent as i32,
                    ProjectAction::DeleteComponent as i32,
                    ProjectAction::CreateWorker as i32,
                    ProjectAction::DeleteProjectGrants as i32,
                    ProjectAction::DeleteApiDefinition as i32,
                    ProjectAction::CreateApiDefinition as i32,
                ],
            };

            let actions = response
                .actions
                .iter()
                .map(|n| {
                    ProjectAction::try_from(*n).map_err(|err| format!("Invalid action ({err})"))
                })
                .collect::<Result<HashSet<ProjectAction>, String>>();

            assert_eq!(
                actions,
                Ok(vec![
                    ProjectAction::ViewWorker,
                    ProjectAction::DeleteWorker,
                    ProjectAction::ViewComponent,
                    ProjectAction::ViewProjectGrants,
                    ProjectAction::ViewApiDefinition,
                    ProjectAction::UpdateWorker,
                    ProjectAction::UpdateApiDefinition,
                    ProjectAction::CreateComponent,
                    ProjectAction::CreateProjectGrants,
                    ProjectAction::UpdateComponent,
                    ProjectAction::DeleteComponent,
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
