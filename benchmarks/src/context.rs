pub mod db;
pub mod golem_template_service;
pub mod golem_worker_service;
pub mod redis;
pub mod routing;
pub mod shard_manager;
pub mod worker;

use crate::context::db::{Db, DbInfo};
use crate::context::golem_template_service::{GolemTemplateService, GolemTemplateServiceInfo};
use crate::context::golem_worker_service::{GolemWorkerService, GolemWorkerServiceInfo};
use crate::context::redis::{Redis, RedisInfo};
use crate::context::shard_manager::{ShardManager, ShardManagerInfo};
use crate::context::worker::{WorkerExecutors, WorkerExecutorsInfo};
use anyhow::Result;
use k8s_openapi::api::core::v1::{Namespace, Pod, Service};
use k8s_openapi::api::networking::v1::Ingress;
use kube::api::{DeleteParams, PostParams};
use kube::{Api, Client};
use serde_json::json;
use std::path::PathBuf;
use std::process::Stdio;
use testcontainers::clients;

const NETWORK: &str = "golem_test_network";
const TAG: &str = "v0.0.63";

#[derive(Debug, Clone)]
pub struct EnvConfig {
    pub verbose: bool,
    pub on_ci: bool,
    pub quiet: bool,
    pub redis_key_prefix: String,
    pub wasm_root: PathBuf,
    pub runtime: Runtime,
    pub db_type: DbType,
    pub n_worker_executors: u16,
    pub schema: String,
}

#[derive(Debug, Clone)]
pub enum DbType {
    Postgres,
    Sqlite,
}

#[derive(Debug, Clone)]
pub enum Runtime {
    Local,
    Docker,
    K8S {
        namespace: K8sNamespace,
        routing: K8sRoutingType,
    },
}

#[derive(Debug, Clone)]
pub struct K8sNamespace(pub String);

#[derive(Debug, Clone)]
pub enum K8sRoutingType {
    Minikube,
    Ingress,
}

impl DbType {
    pub fn from_env() -> DbType {
        let db_type_str = std::env::var("GOLEM_TEST_DB")
            .unwrap_or("".to_string())
            .to_lowercase();
        if db_type_str == "sqlite" {
            DbType::Sqlite
        } else {
            DbType::Postgres
        }
    }
}

pub struct ManagedPod {
    name: String,
    namespace: K8sNamespace,
}

impl ManagedPod {
    pub fn new<S: Into<String>>(name: S, namespace: &K8sNamespace) -> ManagedPod {
        ManagedPod {
            name: name.into(),
            namespace: namespace.clone(),
        }
    }
}

impl Drop for ManagedPod {
    fn drop(&mut self) {
        // println!("Stopping pod {}", self.name);

        // let name = self.name.clone();
        //
        // let stop = async move {
        //     let pods: Api<Pod> =
        //         Api::namespaced(Client::try_default().await.unwrap(), &self.namespace.0);
        //
        //     let dp = DeleteParams::default();
        //     match pods.delete(&name, &dp).await {
        //         Ok(_) => println!("Pod deleted: {name}"),
        //         Err(e) => eprintln!("!!! Failed to delete pod {name}: {e:?}"),
        //     }
        // };
        //
        // futures::executor::block_on(stop);
    }
}

pub struct ManagedService {
    name: String,
    namespace: K8sNamespace,
}

impl ManagedService {
    pub fn new<S: Into<String>>(name: S, namespace: &K8sNamespace) -> ManagedService {
        ManagedService {
            name: name.into(),
            namespace: namespace.clone(),
        }
    }
}

impl Drop for ManagedService {
    fn drop(&mut self) {
        // println!("Stopping service {}", self.name);

        // let name = self.name.clone();
        //
        // let stop = async move {
        //     let services: Api<Service> =
        //         Api::namespaced(Client::try_default().await.unwrap(), &self.namespace.0);
        //
        //     let dp = DeleteParams::default();
        //     match services.delete(&name, &dp).await {
        //         Ok(_) => println!("Service deleted: {name}"),
        //         Err(e) => eprintln!("!!! Failed to delete service {name}: {e:?}"),
        //     }
        // };
        //
        // futures::executor::block_on(stop);
    }
}

pub enum ManagedRouting {
    Minikube,
    Ingress(ManagedIngress),
}

impl ManagedRouting {
    pub fn minikube() -> ManagedRouting {
        ManagedRouting::Minikube
    }
    pub fn ingress<S: Into<String>>(name: S, namespace: &K8sNamespace) -> ManagedRouting {
        ManagedRouting::Ingress(ManagedIngress::new(name, namespace))
    }
}

pub struct ManagedIngress {
    name: String,
    namespace: K8sNamespace,
}

impl ManagedIngress {
    pub fn new<S: Into<String>>(name: S, namespace: &K8sNamespace) -> ManagedIngress {
        ManagedIngress {
            name: name.into(),
            namespace: namespace.clone(),
        }
    }
}

impl Drop for ManagedIngress {
    fn drop(&mut self) {
        // println!("Stopping ingress {}", self.name);

        // let name = self.name.clone();
        //
        // let stop = async move {
        //     let ingresses: Api<Ingress> =
        //         Api::namespaced(Client::try_default().await.unwrap(), &self.namespace.0);
        //
        //     let dp = DeleteParams::default();
        //     match ingresses.delete(&name, &dp).await {
        //         Ok(_) => println!("Ingress deleted: {name} in {:?}", self.namespace),
        //         Err(e) => eprintln!(
        //             "!!! Failed to delete ingress {name} in {:?}: {e:?}",
        //             self.namespace
        //         ),
        //     }
        // };
        //
        // futures::executor::block_on(stop);
    }
}

pub struct ManagedNamespace {
    namespace: K8sNamespace,
}

impl ManagedNamespace {
    pub fn new(namespace: &K8sNamespace) -> ManagedNamespace {
        ManagedNamespace {
            namespace: namespace.clone(),
        }
    }
}

impl Drop for ManagedNamespace {
    fn drop(&mut self) {
        println!("Dropping namespace {}", self.namespace.0);

        let name = self.namespace.0.clone();

        let _ = std::process::Command::new("kubectl")
            .arg("delete")
            .arg("namespaces")
            .arg(&name)
            .stdin(Stdio::null())
            .status()
            .unwrap();
        //
        // let stop = async move {
        //     let namespaces: Api<Namespace> =
        //         Api::all(Client::try_default().await.unwrap());
        //
        //     let dp = DeleteParams::default();
        //     match namespaces.delete(&name, &dp).await {
        //         Ok(_) => println!("Namespace deleted: {name}"),
        //         Err(e) => eprintln!(
        //             "!!! Failed to delete namespace {name} in {:?}: {e:?}",
        //             self.namespace
        //         ),
        //     }
        // };

        // futures::executor::block_on(stop).unwrap();
    }
}

impl EnvConfig {
    pub fn from_env() -> EnvConfig {
        let runtime = if std::env::var("GOLEM_DOCKER_SERVICES").is_err() {
            Runtime::Local
        } else {
            Runtime::Docker
        };

        EnvConfig {
            verbose: std::env::var("CI").is_err(),
            on_ci: std::env::var("CI").is_ok(),
            quiet: std::env::var("QUIET").is_ok(),
            redis_key_prefix: std::env::var("REDIS_KEY_PREFIX").unwrap_or("".to_string()),
            wasm_root: PathBuf::from(
                std::env::var("GOLEM_TEST_TEMPLATES").unwrap_or("../test-templates".to_string()),
            ),
            runtime,
            db_type: DbType::from_env(),
            n_worker_executors: 3,
            schema: "http".to_string(),
        }
    }
}

pub struct Context<'docker_client> {
    env: EnvConfig,
    _namespace: Option<ManagedNamespace>,
    db: Db<'docker_client>,
    redis: Redis<'docker_client>,
    shard_manager: ShardManager<'docker_client>,
    golem_template_service: GolemTemplateService<'docker_client>,
    golem_worker_service: GolemWorkerService<'docker_client>,
    worker_executors: WorkerExecutors<'docker_client>,
}

impl Context<'_> {
    pub async fn start(docker: &clients::Cli, env_config: EnvConfig) -> Result<Context> {
        println!("Starting context with env config: {env_config:?}");

        let namespace = Self::make_namespace(&env_config).await?;
        let db = Db::start(docker, &env_config).await?;
        let redis = Redis::make(docker, &env_config).await?;
        let shard_manager = ShardManager::start(docker, &env_config, &redis.info()).await?;

        let golem_template_service =
            GolemTemplateService::start(docker, &env_config, &db.info()).await?;

        let golem_worker_service = GolemWorkerService::start(
            docker,
            &env_config,
            &shard_manager.info(),
            &db.info(),
            &redis.info(),
            &golem_template_service.info(),
        )
        .await?;

        let worker_executors = WorkerExecutors::start(
            docker,
            &env_config,
            &redis.info(),
            &golem_worker_service.info(),
            &golem_template_service.info(),
            &shard_manager.info(),
        )
        .await?;

        Ok(Context {
            env: env_config,
            _namespace: namespace,
            db,
            redis,
            shard_manager,
            golem_template_service,
            golem_worker_service,
            worker_executors,
        })
    }

    async fn make_namespace(env_config: &EnvConfig) -> Result<Option<ManagedNamespace>> {
        match &env_config.runtime {
            Runtime::K8S { namespace: ns, .. } => {
                println!("Creating Namespace {}", ns.0);

                let namespaces: Api<Namespace> = Api::all(Client::try_default().await?);

                let namespace: Namespace = serde_json::from_value(json!({
                    "apiVersion": "v1",
                    "kind": "Namespace",
                    "metadata": {
                        "name": &ns.0,
                        "labels": {
                            "app-group": "golem"
                        },
                    }
                }))?;

                let pp = PostParams::default();

                let res_ns = namespaces.create(&pp, &namespace).await?;

                println!(
                    "Namespace created: {}",
                    serde_json::to_string_pretty(&res_ns)?
                );

                Ok(Some(ManagedNamespace::new(ns)))
            }
            _ => Ok(None),
        }
    }

    pub fn info(&self) -> ContextInfo {
        ContextInfo {
            env: self.env.clone(),
            db: self.db.info(),
            redis: self.redis.info(),
            shard_manager: self.shard_manager.info(),
            golem_template_service: self.golem_template_service.info(),
            golem_worker_service: self.golem_worker_service.info(),
            worker_executors: self.worker_executors.info(),
        }
    }
}

impl Drop for Context<'_> {
    fn drop(&mut self) {
        println!("Stopping Context")
    }
}

#[derive(Debug, Clone)]
pub struct ContextInfo {
    pub env: EnvConfig,
    pub db: DbInfo,
    pub redis: RedisInfo,
    pub shard_manager: ShardManagerInfo,
    pub golem_template_service: GolemTemplateServiceInfo,
    pub golem_worker_service: GolemWorkerServiceInfo,
    pub worker_executors: WorkerExecutorsInfo,
}
