use std::collections::HashSet;
use std::ops::Deref;
use std::result::Result;
use std::sync::Arc;

use async_trait::async_trait;
use cloud_common::model::ProjectPolicyId;
use cloud_common::model::{ProjectAction, ProjectActions};
use sqlx::{Database, Pool};
use uuid::Uuid;

use crate::model::ProjectPolicy;
use crate::repo::RepoError;

#[derive(sqlx::FromRow, Debug, Clone)]
pub struct ProjectPolicyRecord {
    pub project_policy_id: Uuid,
    pub name: String,
    pub view_component: bool,
    pub create_component: bool,
    pub update_component: bool,
    pub delete_component: bool,
    pub view_worker: bool,
    pub create_worker: bool,
    pub update_worker: bool,
    pub delete_worker: bool,
    pub view_project_grants: bool,
    pub create_project_grants: bool,
    pub delete_project_grants: bool,
    pub view_api_definition: bool,
    pub create_api_definition: bool,
    pub update_api_definition: bool,
    pub delete_api_definition: bool,
}

impl From<ProjectPolicyRecord> for ProjectPolicy {
    fn from(value: ProjectPolicyRecord) -> Self {
        let mut project_actions: HashSet<ProjectAction> = HashSet::new();

        if value.view_component {
            project_actions.insert(ProjectAction::ViewComponent);
        }
        if value.create_component {
            project_actions.insert(ProjectAction::CreateComponent);
        }
        if value.update_component {
            project_actions.insert(ProjectAction::UpdateComponent);
        }
        if value.delete_component {
            project_actions.insert(ProjectAction::DeleteComponent);
        }
        if value.view_worker {
            project_actions.insert(ProjectAction::ViewWorker);
        }
        if value.create_worker {
            project_actions.insert(ProjectAction::CreateWorker);
        }
        if value.update_worker {
            project_actions.insert(ProjectAction::UpdateWorker);
        }
        if value.delete_worker {
            project_actions.insert(ProjectAction::DeleteWorker);
        }
        if value.view_project_grants {
            project_actions.insert(ProjectAction::ViewProjectGrants);
        }
        if value.create_project_grants {
            project_actions.insert(ProjectAction::CreateProjectGrants);
        }
        if value.delete_project_grants {
            project_actions.insert(ProjectAction::DeleteProjectGrants);
        }
        if value.view_api_definition {
            project_actions.insert(ProjectAction::ViewApiDefinition);
        }
        if value.create_api_definition {
            project_actions.insert(ProjectAction::CreateApiDefinition);
        }
        if value.update_api_definition {
            project_actions.insert(ProjectAction::UpdateApiDefinition);
        }
        if value.delete_api_definition {
            project_actions.insert(ProjectAction::DeleteApiDefinition);
        }

        ProjectPolicy {
            id: ProjectPolicyId(value.project_policy_id),
            name: value.name,
            project_actions: ProjectActions {
                actions: project_actions,
            },
        }
    }
}

impl From<ProjectPolicy> for ProjectPolicyRecord {
    fn from(value: ProjectPolicy) -> Self {
        Self {
            project_policy_id: value.id.0,
            name: value.name,
            view_component: value
                .project_actions
                .actions
                .contains(&ProjectAction::ViewComponent),
            create_component: value
                .project_actions
                .actions
                .contains(&ProjectAction::CreateComponent),
            update_component: value
                .project_actions
                .actions
                .contains(&ProjectAction::UpdateComponent),
            delete_component: value
                .project_actions
                .actions
                .contains(&ProjectAction::DeleteComponent),
            view_worker: value
                .project_actions
                .actions
                .contains(&ProjectAction::ViewWorker),
            create_worker: value
                .project_actions
                .actions
                .contains(&ProjectAction::CreateWorker),
            update_worker: value
                .project_actions
                .actions
                .contains(&ProjectAction::UpdateWorker),
            delete_worker: value
                .project_actions
                .actions
                .contains(&ProjectAction::DeleteWorker),
            view_project_grants: value
                .project_actions
                .actions
                .contains(&ProjectAction::ViewProjectGrants),
            create_project_grants: value
                .project_actions
                .actions
                .contains(&ProjectAction::CreateProjectGrants),
            delete_project_grants: value
                .project_actions
                .actions
                .contains(&ProjectAction::DeleteProjectGrants),
            view_api_definition: value
                .project_actions
                .actions
                .contains(&ProjectAction::ViewApiDefinition),
            create_api_definition: value
                .project_actions
                .actions
                .contains(&ProjectAction::CreateApiDefinition),
            update_api_definition: value
                .project_actions
                .actions
                .contains(&ProjectAction::UpdateApiDefinition),
            delete_api_definition: value
                .project_actions
                .actions
                .contains(&ProjectAction::DeleteApiDefinition),
        }
    }
}

#[async_trait]
pub trait ProjectPolicyRepo {
    async fn create(&self, project_policy: &ProjectPolicyRecord) -> Result<(), RepoError>;

    async fn get(&self, project_policy_id: &Uuid)
        -> Result<Option<ProjectPolicyRecord>, RepoError>;

    async fn get_by_name(&self, name: &str) -> Result<Vec<ProjectPolicyRecord>, RepoError>;

    async fn get_all(
        &self,
        project_policy_ids: Vec<Uuid>,
    ) -> Result<Vec<ProjectPolicyRecord>, RepoError>;

    async fn delete(&self, project_policy_id: &Uuid) -> Result<(), RepoError>;
}

pub struct DbProjectPolicyRepo<DB: Database> {
    db_pool: Arc<Pool<DB>>,
}

impl<DB: Database> DbProjectPolicyRepo<DB> {
    pub fn new(db_pool: Arc<Pool<DB>>) -> Self {
        Self { db_pool }
    }
}

#[async_trait]
impl ProjectPolicyRepo for DbProjectPolicyRepo<sqlx::Postgres> {
    async fn create(&self, project_policy: &ProjectPolicyRecord) -> Result<(), RepoError> {
        sqlx::query(
            r#"
              INSERT INTO project_policies
                (
                project_policy_id, name,
                view_component, create_component, update_component, delete_component,
                view_worker, create_worker, update_worker, delete_worker,
                view_project_grants, create_project_grants, delete_project_grants,
                view_api_definition, create_api_definition, update_api_definition, delete_api_definition
                )
              VALUES
                (
                 $1, $2,
                 $3, $4, $5, $6,
                 $7, $8, $9, $10,
                 $11, $12, $13,
                 $14, $15, $16, $17
                )
            "#,
             )
            .bind(project_policy.project_policy_id)
            .bind(project_policy.name.clone())
            .bind(project_policy.view_component)
            .bind(project_policy.create_component)
            .bind(project_policy.update_component)
            .bind(project_policy.delete_component)
            .bind(project_policy.view_worker)
            .bind(project_policy.create_worker)
            .bind(project_policy.update_worker)
            .bind(project_policy.delete_worker)
            .bind(project_policy.view_project_grants)
            .bind(project_policy.create_project_grants)
            .bind(project_policy.delete_project_grants)
            .bind(project_policy.view_api_definition)
            .bind(project_policy.create_api_definition)
            .bind(project_policy.update_api_definition)
            .bind(project_policy.delete_api_definition)
            .execute(self.db_pool.deref())
            .await?;

        Ok(())
    }

    async fn get(
        &self,
        project_policy_id: &Uuid,
    ) -> Result<Option<ProjectPolicyRecord>, RepoError> {
        sqlx::query_as::<_, ProjectPolicyRecord>(
            "SELECT * FROM project_policies WHERE project_policy_id = $1",
        )
        .bind(project_policy_id)
        .fetch_optional(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    async fn get_by_name(&self, name: &str) -> Result<Vec<ProjectPolicyRecord>, RepoError> {
        sqlx::query_as::<_, ProjectPolicyRecord>("SELECT * FROM project_policies WHERE name = $1")
            .bind(name)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_all(
        &self,
        project_policy_ids: Vec<Uuid>,
    ) -> Result<Vec<ProjectPolicyRecord>, RepoError> {
        if project_policy_ids.is_empty() {
            Ok(vec![])
        } else {
            let params = (1..=project_policy_ids.len())
                .map(|i| format!("${}", i))
                .collect::<Vec<_>>()
                .join(", ");
            let query_str = format!(
                "SELECT * FROM project_policies WHERE project_policy_id IN ( { } )",
                params
            );

            let mut query = sqlx::query_as::<_, ProjectPolicyRecord>(&query_str);
            for id in project_policy_ids {
                query = query.bind(id);
            }

            query
                .fetch_all(self.db_pool.deref())
                .await
                .map_err(|e| e.into())
        }
    }

    async fn delete(&self, project_policy_id: &Uuid) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM project_policies WHERE project_policy_id = $1")
            .bind(project_policy_id)
            .execute(self.db_pool.deref())
            .await?;
        Ok(())
    }
}

#[async_trait]
impl ProjectPolicyRepo for DbProjectPolicyRepo<sqlx::Sqlite> {
    async fn create(&self, project_policy: &ProjectPolicyRecord) -> Result<(), RepoError> {
        sqlx::query(
            r#"
              INSERT INTO project_policies
                (
                project_policy_id, name,
                view_component, create_component, update_component, delete_component,
                view_worker, create_worker, update_worker, delete_worker,
                view_project_grants, create_project_grants, delete_project_grants,
                view_api_definition, create_api_definition, update_api_definition, delete_api_definition
                )
              VALUES
                (
                 $1, $2,
                 $3, $4, $5, $6,
                 $7, $8, $9, $10,
                 $11, $12, $13,
                 $14, $15, $16, $17
                )
            "#,
        )
            .bind(project_policy.project_policy_id)
            .bind(project_policy.name.clone())
            .bind(project_policy.view_component)
            .bind(project_policy.create_component)
            .bind(project_policy.update_component)
            .bind(project_policy.delete_component)
            .bind(project_policy.view_worker)
            .bind(project_policy.create_worker)
            .bind(project_policy.update_worker)
            .bind(project_policy.delete_worker)
            .bind(project_policy.view_project_grants)
            .bind(project_policy.create_project_grants)
            .bind(project_policy.delete_project_grants)
            .bind(project_policy.view_api_definition)
            .bind(project_policy.create_api_definition)
            .bind(project_policy.update_api_definition)
            .bind(project_policy.delete_api_definition)
            .execute(self.db_pool.deref())
            .await?;

        Ok(())
    }

    async fn get(
        &self,
        project_policy_id: &Uuid,
    ) -> Result<Option<ProjectPolicyRecord>, RepoError> {
        sqlx::query_as::<_, ProjectPolicyRecord>(
            "SELECT * FROM project_policies WHERE project_policy_id = $1",
        )
        .bind(project_policy_id)
        .fetch_optional(self.db_pool.deref())
        .await
        .map_err(|e| e.into())
    }

    async fn get_by_name(&self, name: &str) -> Result<Vec<ProjectPolicyRecord>, RepoError> {
        sqlx::query_as::<_, ProjectPolicyRecord>("SELECT * FROM project_policies WHERE name = $1")
            .bind(name)
            .fetch_all(self.db_pool.deref())
            .await
            .map_err(|e| e.into())
    }

    async fn get_all(
        &self,
        project_policy_ids: Vec<Uuid>,
    ) -> Result<Vec<ProjectPolicyRecord>, RepoError> {
        if project_policy_ids.is_empty() {
            Ok(vec![])
        } else {
            let params = (1..=project_policy_ids.len())
                .map(|i| format!("${}", i))
                .collect::<Vec<_>>()
                .join(", ");
            let query_str = format!(
                "SELECT * FROM project_policies WHERE project_policy_id IN ( { } )",
                params
            );

            let mut query = sqlx::query_as::<_, ProjectPolicyRecord>(&query_str);
            for id in project_policy_ids {
                query = query.bind(id);
            }

            query
                .fetch_all(self.db_pool.deref())
                .await
                .map_err(|e| e.into())
        }
    }

    async fn delete(&self, project_policy_id: &Uuid) -> Result<(), RepoError> {
        sqlx::query("DELETE FROM project_policies WHERE project_policy_id = $1")
            .bind(project_policy_id)
            .execute(self.db_pool.deref())
            .await?;
        Ok(())
    }
}
