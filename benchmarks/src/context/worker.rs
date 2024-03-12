use crate::context::golem_template_service::GolemTemplateServiceInfo;
use crate::context::golem_worker_service::GolemWorkerServiceInfo;
use crate::context::redis::RedisInfo;
use crate::context::{EnvConfig, ManagedPod, ManagedService, Runtime, ShardManagerInfo, NETWORK, TAG, ManagedRouting, K8sNamespace};
use anyhow::{anyhow, Result};
use k8s_openapi::api::core::v1::{Pod, Service};
use kube::api::PostParams;
use kube::{Api, Client};
use serde_json::json;
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use testcontainers::core::WaitFor;
use testcontainers::{clients, Container, Image, RunnableImage};
use tokio::io::AsyncBufReadExt;
use tokio::io::BufReader;
use tokio::process::{Child, Command};
use tokio::time::timeout;
use tokio::time::Duration;
use tonic::transport::Uri;
use tonic_health::pb::health_check_response::ServingStatus;
use tonic_health::pb::health_client::HealthClient;
use tonic_health::pb::HealthCheckRequest;

pub struct WorkerExecutor<'docker_client> {
    host: String,
    port: u16,
    inner: WorkerExecutorInner<'docker_client>,
}

enum WorkerExecutorInner<'docker_client> {
    Process(Child),
    Docker(Container<'docker_client, WorkerExecutorImage>),
    K8S {
        _pod: ManagedPod,
        _service: ManagedService,
    },
}

#[derive(Debug)]
struct WorkerExecutorImage {
    grpc_port: u16,
    http_port: u16,
    env_vars: HashMap<String, String>,
}

impl WorkerExecutorImage {
    pub fn new(
        grpc_port: u16,
        http_port: u16,
        env_vars: HashMap<String, String>,
    ) -> WorkerExecutorImage {
        WorkerExecutorImage {
            grpc_port,
            http_port,
            env_vars,
        }
    }
}

impl Image for WorkerExecutorImage {
    type Args = ();

    fn name(&self) -> String {
        "golemservices/golem-worker-executor".to_string()
    }

    fn tag(&self) -> String {
        TAG.to_owned()
    }

    fn ready_conditions(&self) -> Vec<WaitFor> {
        vec![WaitFor::message_on_stdout("Registering worker executor")]
    }

    fn env_vars(&self) -> Box<dyn Iterator<Item = (&String, &String)> + '_> {
        Box::new(self.env_vars.iter())
    }

    fn expose_ports(&self) -> Vec<u16> {
        vec![self.grpc_port, self.http_port]
    }
}

impl<'docker_client> WorkerExecutor<'docker_client> {
    pub async fn start(
        docker: &'docker_client clients::Cli,
        shard_id: u16,
        env_config: &EnvConfig,
        redis: &RedisInfo,
        worker: &GolemWorkerServiceInfo,
        template: &GolemTemplateServiceInfo,
        shard_manager: &ShardManagerInfo,
    ) -> Result<WorkerExecutor<'docker_client>> {
        match &env_config.runtime {
            Runtime::Local => {
                WorkerExecutor::start_process(
                    shard_id,
                    env_config,
                    redis,
                    worker,
                    template,
                    shard_manager,
                )
                .await
            }
            Runtime::Docker => WorkerExecutor::start_docker(
                docker,
                shard_id,
                env_config,
                redis,
                worker,
                template,
                shard_manager,
            ),
            Runtime::K8S{namespace, routing: _} => {
                WorkerExecutor::start_k8s(
                    shard_id,
                    env_config,
                    redis,
                    worker,
                    template,
                    shard_manager,
                    namespace,
                )
                .await
            }
        }
    }

    async fn start_k8s(
        shard_id: u16,
        env_config: &EnvConfig,
        redis: &RedisInfo,
        worker: &GolemWorkerServiceInfo,
        template: &GolemTemplateServiceInfo,
        shard_manager: &ShardManagerInfo,
        namespace: &K8sNamespace,
    ) -> Result<WorkerExecutor<'docker_client>> {
        println!("Starting Worker Executor {shard_id} pod");

        let port = 9000 + shard_id;
        let http_port = 9100 + shard_id;

        let env_vars = WorkerExecutor::env_vars(
            port,
            http_port,
            env_config,
            redis,
            worker,
            template,
            shard_manager,
        );
        let env_vars = env_vars
            .into_iter()
            .map(|(k, v)| json!({"name": k, "value": v}))
            .collect::<Vec<_>>();
        let name = &format!("golem-worker-executor{shard_id}");

        let pods: Api<Pod> = Api::namespaced(Client::try_default().await?, &namespace.0);
        let services: Api<Service> = Api::namespaced(Client::try_default().await?, &namespace.0);

        let pod: Pod = serde_json::from_value(json!({
            "apiVersion": "v1",
            "kind": "Pod",
            "metadata": {
                "name": &name,
                "labels": {
                    "app": &name,
                    "app-group": "golem"
                },
            },
            "spec": {
                "ports": [
                    {
                        "port": port,
                        "protocol": "TCP"
                    },
                    {
                        "port": http_port,
                        "protocol": "TCP"
                    }
                ],
                "containers": [{
                    "name": "service",
                    "image": format!("golemservices/golem-worker-executor:{}", TAG),
                    "env": env_vars
                }]
            }
        }))?;

        let pp = PostParams::default();

        let res_pod = pods.create(&pp, &pod).await?;

        let managed_pod = ManagedPod::new(name, namespace);

        let service: Service = serde_json::from_value(json!({
            "apiVersion": "v1",
            "kind": "Service",
            "metadata": {
                "name": name,
                "labels": {
                    "app": name,
                    "app-group": "golem"
                },
            },
            "spec": {
                "ports": [
                    {
                        "name": "grpc",
                        "port": port,
                        "protocol": "TCP"
                    },
                    {
                        "name": "http",
                        "port": http_port,
                        "protocol": "TCP"
                    }
                ],
                "selector": { "app": name },
                "type": "LoadBalancer"
            }
        }))?;

        let res_srv = services.create(&pp, &service).await?;

        let managed_service = ManagedService::new(name, namespace);

        Ok(WorkerExecutor {
            host: format!("{name}.{}.svc.cluster.local", &namespace.0),
            port,
            inner: WorkerExecutorInner::K8S {
                _pod: managed_pod,
                _service: managed_service,
            },
        })
    }

    fn start_docker(
        docker: &'docker_client clients::Cli,
        shard_id: u16,
        env_config: &EnvConfig,
        redis: &RedisInfo,
        worker: &GolemWorkerServiceInfo,
        template: &GolemTemplateServiceInfo,
        shard_manager: &ShardManagerInfo,
    ) -> Result<WorkerExecutor<'docker_client>> {
        println!("Starting Worker Executor {shard_id} docker");

        let port = 9000 + shard_id;
        let http_port = 9100 + shard_id;

        let env_vars = WorkerExecutor::env_vars(
            port,
            http_port,
            env_config,
            redis,
            worker,
            template,
            shard_manager,
        );
        let name = format!("golem_worker_executor{shard_id}");

        let image = RunnableImage::from(WorkerExecutorImage::new(port, http_port, env_vars))
            .with_container_name(&name)
            .with_network(NETWORK);
        let node = docker.run(image);

        Ok(WorkerExecutor {
            host: name,
            port,
            inner: WorkerExecutorInner::Docker(node),
        })
    }

    fn env_vars(
        port: u16,
        http_port: u16,
        env_config: &EnvConfig,
        redis: &RedisInfo,
        worker: &GolemWorkerServiceInfo,
        template: &GolemTemplateServiceInfo,
        shard_manager: &ShardManagerInfo,
    ) -> HashMap<String, String> {
        let log_level = if env_config.verbose { "debug" } else { "info" };

        let env: &[(&str, &str)] = &[
            ("RUST_LOG"                                      , &format!("{log_level},cranelift_codegen=warn,wasmtime_cranelift=warn,wasmtime_jit=warn,h2=warn,hyper=warn,tower=warn")),
            ("ENVIRONMENT"                    , "local"),
            ("WASMTIME_BACKTRACE_DETAILS"                    , "1"),
            ("RUST_BACKTRACE"                                , "1"),
            ("GOLEM__REDIS__HOST"                            , &redis.host),
            ("GOLEM__REDIS__PORT"                            , &redis.port.to_string()),
            ("GOLEM__REDIS__KEY_PREFIX"                      , &env_config.redis_key_prefix),
            ("GOLEM__PUBLIC_WORKER_API__HOST"                , &worker.host),
            ("GOLEM__PUBLIC_WORKER_API__PORT"                , &worker.grpc_port.to_string()),
            ("GOLEM__PUBLIC_WORKER_API__ACCESS_TOKEN"        , "2A354594-7A63-4091-A46B-CC58D379F677"),
            ("GOLEM__TEMPLATE_SERVICE__CONFIG__HOST"         , &template.host),
            ("GOLEM__TEMPLATE_SERVICE__CONFIG__PORT"         , &template.grpc_port.to_string()),
            ("GOLEM__TEMPLATE_SERVICE__CONFIG__ACCESS_TOKEN" , "2A354594-7A63-4091-A46B-CC58D379F677"),
            ("GOLEM__COMPILED_TEMPLATE_SERVICE__TYPE"        , "Disabled"),
            // ("GOLEM__COMPILED_TEMPLATE_SERVICE__TYPE"        , "Local"),
            // ("GOLEM__COMPILED_TEMPLATE_SERVICE__CONFIG__ROOT"        , "data/templates"),
            ("GOLEM__BLOB_STORE_SERVICE__TYPE"               , "InMemory"),
            ("GOLEM__SHARD_MANAGER_SERVICE__TYPE"               , "Grpc"),
            ("GOLEM__SHARD_MANAGER_SERVICE__CONFIG__HOST"    , &shard_manager.host),
            ("GOLEM__SHARD_MANAGER_SERVICE__CONFIG__PORT"    , &shard_manager.grpc_port.to_string()),
            ("GOLEM__SHARD_MANAGER_SERVICE__CONFIG__RETRIES__MAX_ATTEMPTS"    , "5"),
            ("GOLEM__SHARD_MANAGER_SERVICE__CONFIG__RETRIES__MIN_DELAY"    , "100ms"),
            ("GOLEM__SHARD_MANAGER_SERVICE__CONFIG__RETRIES__MAX_DELAY"    , "2s"),
            ("GOLEM__SHARD_MANAGER_SERVICE__CONFIG__RETRIES__MULTIPLIER"    , "2"),
            ("GOLEM__PORT"                                   , &port.to_string()),
            ("GOLEM__HTTP_PORT"                              , &http_port.to_string()),
        ];

        env.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    async fn start_process(
        shard_id: u16,
        env_config: &EnvConfig,
        redis: &RedisInfo,
        worker: &GolemWorkerServiceInfo,
        template: &GolemTemplateServiceInfo,
        shard_manager: &ShardManagerInfo,
    ) -> Result<WorkerExecutor<'docker_client>> {
        let port = 9000 + shard_id;
        let http_port = 9100 + shard_id;

        println!("Starting Worker Executor {shard_id}");

        let service_dir = Path::new("../golem-worker-executor/");
        let service_run = Path::new("../target/debug/worker-executor");

        if !service_run.exists() {
            return Err(anyhow!(
                "Expected to have precompiled Worker Executor at {}",
                service_run.to_str().unwrap_or("")
            ));
        }

        println!("Starting {}", service_run.to_str().unwrap_or(""));

        let mut child = Command::new(service_run)
            .current_dir(service_dir)
            .envs(WorkerExecutor::env_vars(
                port,
                http_port,
                env_config,
                redis,
                worker,
                template,
                shard_manager,
            ))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let quiet = env_config.quiet;

        let stdout = child
            .stdout
            .take()
            .ok_or(anyhow!("Can't get Worker Executor stdout"))?;

        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();

            while let Some(line) = lines.next_line().await.unwrap() {
                if !quiet {
                    println!("[workerexec{shard_id}]   {}", line)
                }
            }
        });

        let stderr = child
            .stderr
            .take()
            .ok_or(anyhow!("Can't get Worker Executor stderr"))?;

        tokio::spawn(async move {
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();

            while let Some(line) = lines.next_line().await.unwrap() {
                if !quiet {
                    println!("[workerexec{shard_id}]   {}", line)
                }
            }
        });

        WorkerExecutor::wait_for_health_check(port).await;

        println!("Worker Executor {shard_id} online");

        Ok(WorkerExecutor {
            host: "localhost".to_string(),
            port,
            inner: WorkerExecutorInner::Process(child),
        })
    }

    async fn wait_for_health_check(port: u16) {
        loop {
            if WorkerExecutor::health_check_address("localhost", port).await {
                break;
            } else {
                println!("Worker Executor healthcheck failed.");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }

    async fn health_check_address(host: &str, port: u16) -> bool {
        let address = format!("http://{host}:{port}");

        println!("Worker executor health checking on {address}");
        match address.parse::<Uri>() {
            Ok(uri) => {
                let conn = timeout(
                    Duration::from_secs(2),
                    tonic::transport::Endpoint::from(uri).connect(),
                )
                .await;

                match conn {
                    Ok(conn) => match conn {
                        Ok(conn) => {
                            let request = HealthCheckRequest {
                                service: "".to_string(),
                            };
                            match HealthClient::new(conn).check(request).await {
                                Ok(response) => {
                                    response.into_inner().status == ServingStatus::Serving as i32
                                }
                                Err(err) => {
                                    println!("Health request returned with an error: {:?}", err);
                                    false
                                }
                            }
                        }
                        Err(err) => {
                            println!("Failed to connect to {address}: {:?}", err);
                            false
                        }
                    },
                    Err(_) => {
                        println!("Connection to {address} timed out");
                        false
                    }
                }
            }
            Err(_) => {
                println!("Worker executor has an invalid URI: {address}");
                false
            }
        }
    }

    pub fn info(&self) -> WorkerExecutorInfo {
        WorkerExecutorInfo {
            host: self.host.clone(),
            port: self.port,
        }
    }
}

impl Drop for WorkerExecutor<'_> {
    fn drop(&mut self) {
        match &mut self.inner {
            WorkerExecutorInner::Process(child) => {
                // TODO: SIGTERM, maybe with nix crate.
                match futures::executor::block_on(child.kill()) {
                    Ok(_) => println!("Killed Worker Executor (port {}) process", self.port),
                    Err(e) => eprintln!(
                        "Failed to kill Worker Executor (port {}) process: {e:? }",
                        self.port
                    ),
                }
            }
            WorkerExecutorInner::Docker(_) => {}
            WorkerExecutorInner::K8S { .. } => {}
        }

        println!("Stopped Worker Executor (port {})", self.port)
    }
}

pub struct WorkerExecutors<'docker_client> {
    worker_executors: Vec<WorkerExecutor<'docker_client>>,
}

impl<'docker_client> WorkerExecutors<'docker_client> {
    pub async fn start(
        docker: &'docker_client clients::Cli,
        env_config: &EnvConfig,
        redis: &RedisInfo,
        worker: &GolemWorkerServiceInfo,
        template: &GolemTemplateServiceInfo,
        shard_manager: &ShardManagerInfo,
    ) -> Result<WorkerExecutors<'docker_client>> {
        let shards = 3;

        let mut worker_executors = Vec::with_capacity(shards);

        for shard_id in 0..env_config.n_worker_executors {
            worker_executors.push(
                WorkerExecutor::start(
                    docker,
                    shard_id.try_into().unwrap(),
                    env_config,
                    redis,
                    worker,
                    template,
                    shard_manager,
                )
                .await?,
            )
        }

        Ok(WorkerExecutors { worker_executors })
    }

    pub fn info(&self) -> WorkerExecutorsInfo {
        WorkerExecutorsInfo {
            worker_executors: self.worker_executors.iter().map(|we| we.info()).collect(),
        }
    }
}

impl Drop for WorkerExecutors<'_> {
    fn drop(&mut self) {
        println!("Stopping {} worker executors", self.worker_executors.len())
    }
}

#[derive(Debug, Clone)]
pub struct WorkerExecutorInfo {
    host: String,
    pub port: u16,
}

impl WorkerExecutorInfo {
    pub fn health_check(&self) -> bool {
        let result = WorkerExecutor::health_check_address(&self.host, self.port);

        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed building the Runtime")
            .block_on(result)
    }
}

#[derive(Debug, Clone)]
pub struct WorkerExecutorsInfo {
    pub worker_executors: Vec<WorkerExecutorInfo>,
}
