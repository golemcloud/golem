use crate::context::golem_template_service::GolemTemplateServiceInfo;
use crate::context::golem_worker_service::GolemWorkerServiceInfo;
use crate::context::redis::RedisInfo;
use crate::context::{EnvConfig, ShardManagerInfo, NETWORK, TAG};
use libtest_mimic::Failed;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use testcontainers::core::WaitFor;
use testcontainers::{clients, Container, Image, RunnableImage};
use tokio::time::timeout;
use tonic::transport::Uri;
use tonic_health::pb::health_check_response::ServingStatus;
use tonic_health::pb::health_client::HealthClient;
use tonic_health::pb::HealthCheckRequest;

pub struct WorkerExecutor<'docker_client> {
    pub shard_id: u16,
    host: String,
    port: u16,
    inner: WorkerExecutorInner<'docker_client>,
}

enum WorkerExecutorInner<'docker_client> {
    Process(Child),
    Docker(Container<'docker_client, WorkerExecutorImage>),
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
    pub fn start(
        docker: &'docker_client clients::Cli,
        shard_id: u16,
        env_config: &EnvConfig,
        redis: &RedisInfo,
        worker: &GolemWorkerServiceInfo,
        template: &GolemTemplateServiceInfo,
        shard_manager: &ShardManagerInfo,
        wait_for_health: bool,
    ) -> Result<WorkerExecutor<'docker_client>, Failed> {
        if env_config.local_golem {
            WorkerExecutor::start_process(
                shard_id,
                env_config,
                redis,
                worker,
                template,
                shard_manager,
                wait_for_health,
            )
        } else {
            WorkerExecutor::start_docker(
                docker,
                shard_id,
                env_config,
                redis,
                worker,
                template,
                shard_manager,
            )
        }
    }

    fn start_docker(
        docker: &'docker_client clients::Cli,
        shard_id: u16,
        env_config: &EnvConfig,
        redis: &RedisInfo,
        worker: &GolemWorkerServiceInfo,
        template: &GolemTemplateServiceInfo,
        shard_manager: &ShardManagerInfo,
    ) -> Result<WorkerExecutor<'docker_client>, Failed> {
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
            shard_id,
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

    fn start_process(
        shard_id: u16,
        env_config: &EnvConfig,
        redis: &RedisInfo,
        worker: &GolemWorkerServiceInfo,
        template: &GolemTemplateServiceInfo,
        shard_manager: &ShardManagerInfo,
        wait_for_health: bool,
    ) -> Result<WorkerExecutor<'docker_client>, Failed> {
        let port = 9000 + shard_id;
        let http_port = 9100 + shard_id;

        println!("Starting Worker Executor {shard_id}");

        let service_dir = Path::new("../golem-worker-executor/");
        let service_run = Path::new("../target/debug/worker-executor");

        if !service_run.exists() {
            return Err(format!(
                "Expected to have precompiled Worker Executor at {}",
                service_run.to_str().unwrap_or("")
            )
            .into());
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
            .ok_or::<Failed>("Can't get Worker Executor stdout".into())?;

        // TODO: use tokio
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                if !quiet {
                    println!("[workerexec{shard_id}]   {}", line.unwrap())
                }
            }
        });

        let stderr = child
            .stderr
            .take()
            .ok_or::<Failed>("Can't get Worker Executor stderr".into())?;

        std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                if !quiet {
                    eprintln!("[workerexec{shard_id}]   {}", line.unwrap())
                }
            }
        });

        if wait_for_health {
            WorkerExecutor::wait_for_health_check(port);
        }

        println!("Worker Executor {shard_id} online");

        Ok(WorkerExecutor {
            shard_id,
            host: "localhost".to_string(),
            port,
            inner: WorkerExecutorInner::Process(child),
        })
    }

    fn wait_for_health_check(port: u16) {
        let wait_loop = async {
            loop {
                if WorkerExecutor::health_check_address("localhost", port).await {
                    break;
                } else {
                    println!("Worker Executor healthcheck failed.");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        };

        // TODO: async start
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("Failed building the Runtime")
            .block_on(wait_loop)
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
                match child.kill() {
                    Ok(_) => println!("Killed Worker Executor (port {}) process", self.port),
                    Err(e) => eprintln!(
                        "Failed to kill Worker Executor (port {}) process: {e:? }",
                        self.port
                    ),
                }
            }
            WorkerExecutorInner::Docker(_) => {}
        }

        println!("Stopped Worker Executor (port {})", self.port)
    }
}

pub struct WorkerExecutors<'docker_client> {
    pub worker_executors: Vec<WorkerExecutor<'docker_client>>,
}

impl<'docker_client> WorkerExecutors<'docker_client> {
    pub fn start(
        docker: &'docker_client clients::Cli,
        env_config: &EnvConfig,
        redis: &RedisInfo,
        worker: &GolemWorkerServiceInfo,
        template: &GolemTemplateServiceInfo,
        shard_manager: &ShardManagerInfo,
    ) -> Result<WorkerExecutors<'docker_client>, Failed> {
        let shards = env_config.n_worker_executors;

        let mut worker_executors = Vec::with_capacity(shards);

        for shard_id in 0..shards {
            worker_executors.push(WorkerExecutor::start(
                docker,
                shard_id.try_into().unwrap(),
                env_config,
                redis,
                worker,
                template,
                shard_manager,
                true,
            )?)
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

#[derive(Debug)]
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

#[derive(Debug)]
pub struct WorkerExecutorsInfo {
    pub worker_executors: Vec<WorkerExecutorInfo>,
}
