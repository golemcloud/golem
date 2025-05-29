use crate::model::ProjectPolicy;
use async_trait::async_trait;
use cloud_common::model::ProjectActions;
use cloud_common::model::{ProjectPermisison, ProjectPolicyId};
use conditional_trait_gen::trait_gen;
use golem_service_base::db::Pool;
use golem_service_base::repo::RepoError;
use std::collections::HashSet;
use std::result::Result;
use uuid::Uuid;

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
    pub delete_project: bool,
    pub view_plugin_installations: bool,
    pub create_plugin_installation: bool,
    pub update_plugin_installation: bool,
    pub delete_plugin_installation: bool,
    pub upsert_api_deployment: bool,
    pub view_api_deployment: bool,
    pub delete_api_deployment: bool,
    pub upsert_api_domain: bool,
    pub view_api_domain: bool,
    pub delete_api_domain: bool,
}

impl From<ProjectPolicyRecord> for ProjectPolicy {
    fn from(value: ProjectPolicyRecord) -> Self {
        let mut project_actions: HashSet<ProjectPermisison> = HashSet::new();

        if value.view_component {
            project_actions.insert(ProjectPermisison::ViewComponent);
        }
        if value.create_component {
            project_actions.insert(ProjectPermisison::CreateComponent);
        }
        if value.update_component {
            project_actions.insert(ProjectPermisison::UpdateComponent);
        }
        if value.delete_component {
            project_actions.insert(ProjectPermisison::DeleteComponent);
        }
        if value.view_worker {
            project_actions.insert(ProjectPermisison::ViewWorker);
        }
        if value.create_worker {
            project_actions.insert(ProjectPermisison::CreateWorker);
        }
        if value.update_worker {
            project_actions.insert(ProjectPermisison::UpdateWorker);
        }
        if value.delete_worker {
            project_actions.insert(ProjectPermisison::DeleteWorker);
        }
        if value.view_project_grants {
            project_actions.insert(ProjectPermisison::ViewProjectGrants);
        }
        if value.create_project_grants {
            project_actions.insert(ProjectPermisison::CreateProjectGrants);
        }
        if value.delete_project_grants {
            project_actions.insert(ProjectPermisison::DeleteProjectGrants);
        }
        if value.view_api_definition {
            project_actions.insert(ProjectPermisison::ViewApiDefinition);
        }
        if value.create_api_definition {
            project_actions.insert(ProjectPermisison::CreateApiDefinition);
        }
        if value.update_api_definition {
            project_actions.insert(ProjectPermisison::UpdateApiDefinition);
        }
        if value.delete_api_definition {
            project_actions.insert(ProjectPermisison::DeleteApiDefinition);
        }
        if value.delete_project {
            project_actions.insert(ProjectPermisison::DeleteProject);
        }
        if value.view_plugin_installations {
            project_actions.insert(ProjectPermisison::ViewPluginInstallations);
        }
        if value.create_plugin_installation {
            project_actions.insert(ProjectPermisison::CreatePluginInstallation);
        }
        if value.update_plugin_installation {
            project_actions.insert(ProjectPermisison::UpdatePluginInstallation);
        }
        if value.delete_plugin_installation {
            project_actions.insert(ProjectPermisison::DeletePluginInstallation);
        }
        if value.upsert_api_deployment {
            project_actions.insert(ProjectPermisison::UpsertApiDeployment);
        }
        if value.view_api_deployment {
            project_actions.insert(ProjectPermisison::ViewApiDeployment);
        }
        if value.delete_api_deployment {
            project_actions.insert(ProjectPermisison::DeleteApiDeployment);
        }
        if value.upsert_api_domain {
            project_actions.insert(ProjectPermisison::UpsertApiDomain);
        }
        if value.view_api_domain {
            project_actions.insert(ProjectPermisison::ViewApiDomain);
        }
        if value.delete_api_domain {
            project_actions.insert(ProjectPermisison::DeleteApiDomain);
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
                .contains(&ProjectPermisison::ViewComponent),
            create_component: value
                .project_actions
                .actions
                .contains(&ProjectPermisison::CreateComponent),
            update_component: value
                .project_actions
                .actions
                .contains(&ProjectPermisison::UpdateComponent),
            delete_component: value
                .project_actions
                .actions
                .contains(&ProjectPermisison::DeleteComponent),
            view_worker: value
                .project_actions
                .actions
                .contains(&ProjectPermisison::ViewWorker),
            create_worker: value
                .project_actions
                .actions
                .contains(&ProjectPermisison::CreateWorker),
            update_worker: value
                .project_actions
                .actions
                .contains(&ProjectPermisison::UpdateWorker),
            delete_worker: value
                .project_actions
                .actions
                .contains(&ProjectPermisison::DeleteWorker),
            view_project_grants: value
                .project_actions
                .actions
                .contains(&ProjectPermisison::ViewProjectGrants),
            create_project_grants: value
                .project_actions
                .actions
                .contains(&ProjectPermisison::CreateProjectGrants),
            delete_project_grants: value
                .project_actions
                .actions
                .contains(&ProjectPermisison::DeleteProjectGrants),
            view_api_definition: value
                .project_actions
                .actions
                .contains(&ProjectPermisison::ViewApiDefinition),
            create_api_definition: value
                .project_actions
                .actions
                .contains(&ProjectPermisison::CreateApiDefinition),
            update_api_definition: value
                .project_actions
                .actions
                .contains(&ProjectPermisison::UpdateApiDefinition),
            delete_api_definition: value
                .project_actions
                .actions
                .contains(&ProjectPermisison::DeleteApiDefinition),
            delete_project: value
                .project_actions
                .actions
                .contains(&ProjectPermisison::DeleteProject),
            view_plugin_installations: value
                .project_actions
                .actions
                .contains(&ProjectPermisison::ViewPluginInstallations),
            create_plugin_installation: value
                .project_actions
                .actions
                .contains(&ProjectPermisison::CreatePluginInstallation),
            update_plugin_installation: value
                .project_actions
                .actions
                .contains(&ProjectPermisison::UpdatePluginInstallation),
            delete_plugin_installation: value
                .project_actions
                .actions
                .contains(&ProjectPermisison::DeletePluginInstallation),
            upsert_api_deployment: value
                .project_actions
                .actions
                .contains(&ProjectPermisison::UpsertApiDeployment),
            view_api_deployment: value
                .project_actions
                .actions
                .contains(&ProjectPermisison::ViewApiDeployment),
            delete_api_deployment: value
                .project_actions
                .actions
                .contains(&ProjectPermisison::DeleteApiDeployment),
            upsert_api_domain: value
                .project_actions
                .actions
                .contains(&ProjectPermisison::UpsertApiDomain),
            view_api_domain: value
                .project_actions
                .actions
                .contains(&ProjectPermisison::ViewApiDomain),
            delete_api_domain: value
                .project_actions
                .actions
                .contains(&ProjectPermisison::DeleteApiDomain),
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

pub struct DbProjectPolicyRepo<DB: Pool> {
    db_pool: DB,
}

impl<DB: Pool> DbProjectPolicyRepo<DB> {
    pub fn new(db_pool: DB) -> Self {
        Self { db_pool }
    }
}

#[trait_gen(golem_service_base::db::postgres::PostgresPool -> golem_service_base::db::postgres::PostgresPool, golem_service_base::db::sqlite::SqlitePool
)]
#[async_trait]
impl ProjectPolicyRepo for DbProjectPolicyRepo<golem_service_base::db::postgres::PostgresPool> {
    async fn create(&self, project_policy: &ProjectPolicyRecord) -> Result<(), RepoError> {
        let query = sqlx::query(
            r#"
              INSERT INTO project_policies
                (
                project_policy_id, name, view_component, create_component,
                update_component, delete_component, view_worker, create_worker,
                update_worker, delete_worker, view_project_grants, create_project_grants,
                delete_project_grants, view_api_definition, create_api_definition, update_api_definition,
                delete_api_definition, delete_project, view_plugin_installations, create_plugin_installation,
                update_plugin_installation, delete_plugin_installation, upsert_api_deployment, view_api_deployment,
                delete_api_deployment, upsert_api_domain, view_api_domain, delete_api_domain
                )
              VALUES
                (
                 $1, $2, $3, $4,
                 $5, $6, $7, $8,
                 $9, $10, $11, $12,
                 $13, $14, $15, $16,
                 $17, $18, $19, $20,
                 $21, $22, $23, $24,
                 $25, $26, $27, $28
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
            .bind(project_policy.delete_project)
            .bind(project_policy.view_plugin_installations)
            .bind(project_policy.create_plugin_installation)
            .bind(project_policy.update_plugin_installation)
            .bind(project_policy.delete_plugin_installation)
            .bind(project_policy.upsert_api_deployment)
            .bind(project_policy.view_api_deployment)
            .bind(project_policy.delete_api_deployment)
            .bind(project_policy.upsert_api_domain)
            .bind(project_policy.view_api_domain)
            .bind(project_policy.delete_api_domain);

        self.db_pool
            .with_rw("project_policy", "create")
            .execute(query)
            .await?;

        Ok(())
    }

    async fn get(
        &self,
        project_policy_id: &Uuid,
    ) -> Result<Option<ProjectPolicyRecord>, RepoError> {
        let query = sqlx::query_as::<_, ProjectPolicyRecord>(
            "SELECT * FROM project_policies WHERE project_policy_id = $1",
        )
        .bind(project_policy_id);

        self.db_pool
            .with_ro("project_policy", "get")
            .fetch_optional_as(query)
            .await
    }

    async fn get_by_name(&self, name: &str) -> Result<Vec<ProjectPolicyRecord>, RepoError> {
        let query = sqlx::query_as::<_, ProjectPolicyRecord>(
            "SELECT * FROM project_policies WHERE name = $1",
        )
        .bind(name);

        self.db_pool
            .with_ro("project_policy", "get_by_name")
            .fetch_all(query)
            .await
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

            self.db_pool
                .with_ro("project_policy", "get_all")
                .fetch_all(query)
                .await
        }
    }

    async fn delete(&self, project_policy_id: &Uuid) -> Result<(), RepoError> {
        let query = sqlx::query("DELETE FROM project_policies WHERE project_policy_id = $1")
            .bind(project_policy_id);

        self.db_pool
            .with_rw("project_policy", "delete")
            .execute(query)
            .await?;

        Ok(())
    }
}
