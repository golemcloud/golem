use crate::context::redis::RedisInfo;
use crate::context::{EnvConfig, K8sNamespace, ManagedPod, ManagedService, Runtime, NETWORK, TAG};
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
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

pub struct ShardManager<'docker_client> {
    host: String,
    http_port: u16,
    grpc_port: u16,
    inner: ShardManagerInner<'docker_client>,
}

enum ShardManagerInner<'docker_client> {
    Process(Child),
    Docker(Container<'docker_client, ShardManagerImage>),
    K8S {
        _pod: ManagedPod,
        _service: ManagedService,
    },
}

#[derive(Debug)]
struct ShardManagerImage {
    port: u16,
    http_port: u16,
    env_vars: HashMap<String, String>,
}

impl ShardManagerImage {
    pub fn new(port: u16, http_port: u16, env_vars: HashMap<String, String>) -> ShardManagerImage {
        ShardManagerImage {
            port,
            http_port,
            env_vars,
        }
    }
}

impl Image for ShardManagerImage {
    type Args = ();

    fn name(&self) -> String {
        "golemservices/golem-shard-manager".to_string()
    }

    fn tag(&self) -> String {
        TAG.to_owned()
    }

    fn ready_conditions(&self) -> Vec<WaitFor> {
        vec![WaitFor::message_on_stdout(
            "Shard Manager is fully operational",
        )]
    }

    fn env_vars(&self) -> Box<dyn Iterator<Item = (&String, &String)> + '_> {
        Box::new(self.env_vars.iter())
    }

    fn expose_ports(&self) -> Vec<u16> {
        vec![self.port, self.http_port]
    }
}

impl<'docker_client> ShardManager<'docker_client> {
    pub async fn start(
        docker: &'docker_client clients::Cli,
        env_config: &EnvConfig,
        redis: &RedisInfo,
    ) -> Result<ShardManager<'docker_client>> {
        match &env_config.runtime {
            Runtime::Local => ShardManager::start_process(env_config, redis).await,
            Runtime::Docker => ShardManager::start_docker(docker, env_config, redis),
            Runtime::K8S {
                namespace,
                routing: _,
            } => ShardManager::start_k8s(env_config, redis, namespace).await,
        }
    }

    async fn start_k8s(
        env_config: &EnvConfig,
        redis: &RedisInfo,
        namespace: &K8sNamespace,
    ) -> Result<ShardManager<'docker_client>> {
        let port = 9020;
        let http_port = 9021;
        println!("Starting Golem Shard Manager pod");

        let env_vars = ShardManager::env_vars(port, http_port, env_config, redis);
        let env_vars = env_vars
            .into_iter()
            .map(|(k, v)| json!({"name": k, "value": v}))
            .collect::<Vec<_>>();
        let name = "golem-shard-manager";

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
                    "image": format!("golemservices/golem-shard-manager:{}", TAG),
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
                "selector": { "app": name }
            }
        }))?;

        let res_srv = services.create(&pp, &service).await?;

        let managed_service = ManagedService::new(name, namespace);

        Ok(ShardManager {
            host: format!("{name}.{}.svc.cluster.local", &namespace.0),
            grpc_port: port,
            http_port,
            inner: ShardManagerInner::K8S {
                _pod: managed_pod,
                _service: managed_service,
            },
        })
    }

    fn start_docker(
        docker: &'docker_client clients::Cli,
        env_config: &EnvConfig,
        redis: &RedisInfo,
    ) -> Result<ShardManager<'docker_client>> {
        let port = 9020;
        let http_port = 9021;
        println!("Starting Golem Shard Manager in docker");

        let env_vars = ShardManager::env_vars(port, http_port, env_config, redis);
        let name = "golem_shard_manager";

        let image = RunnableImage::from(ShardManagerImage::new(port, http_port, env_vars))
            .with_container_name(name)
            .with_network(NETWORK);
        let node = docker.run(image);

        println!("Started Golem Shard Manager in docker");

        Ok(ShardManager {
            host: name.to_string(),
            http_port,
            grpc_port: port,
            inner: ShardManagerInner::Docker(node),
        })
    }

    fn env_vars(
        port: u16,
        http_port: u16,
        env_config: &EnvConfig,
        redis: &RedisInfo,
    ) -> HashMap<String, String> {
        let log_level = if env_config.verbose { "debug" } else { "info" };

        let env: &[(&str, &str)] = &[
            ("RUST_LOG", &format!("{log_level},h2=warn,cranelift_codegen=warn,wasmtime_cranelift=warn,wasmtime_jit=warn")),
            ("WASMTIME_BACKTRACE_DETAILS", "1"),
            ("RUST_BACKTRACE", "1"),
            ("REDIS__HOST", &redis.host),
            ("GOLEM__REDIS__HOST", &redis.host),
            ("GOLEM__REDIS__PORT", &redis.port.to_string()),
            ("GOLEM__REDIS__KEY_PREFIX", &env_config.redis_key_prefix),
            ("GOLEM_SHARD_MANAGER_PORT", &port.to_string()),
            ("GOLEM__HTTP_PORT", &http_port.to_string()),
        ];

        env.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    async fn start_process(
        env_config: &EnvConfig,
        redis: &RedisInfo,
    ) -> Result<ShardManager<'docker_client>> {
        let port = 9020;
        let http_port = 9021;
        println!("Starting Golem Shard Manager");

        let service_dir = Path::new("../golem-shard-manager/");
        let service_run = Path::new("../target/debug/golem-shard-manager");

        if !service_run.exists() {
            return Err(anyhow!(
                "Expected to have precompiled Cloud Shard Manager at {}",
                service_run.to_str().unwrap_or("")
            ));
        }

        println!("Starting {}", service_run.to_str().unwrap_or(""));

        let mut child = Command::new(service_run)
            .current_dir(service_dir)
            .envs(ShardManager::env_vars(port, http_port, env_config, redis))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let quiet = env_config.quiet;

        let stdout = child
            .stdout
            .take()
            .ok_or(anyhow!("Can't get shard service stdout"))?;

        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();

            while let Some(line) = lines.next_line().await.unwrap() {
                if !quiet {
                    println!("[shardsvc]   {}", line)
                }
            }
        });

        let stderr = child
            .stderr
            .take()
            .ok_or(anyhow!("Can't get shard service stderr"))?;

        tokio::spawn(async move {
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();

            while let Some(line) = lines.next_line().await.unwrap() {
                if !quiet {
                    println!("[shardsvc]   {}", line)
                }
            }
        });

        Ok(ShardManager {
            host: "localhost".to_string(),
            grpc_port: port,
            http_port,
            inner: ShardManagerInner::Process(child),
        })
    }

    pub fn info(&self) -> ShardManagerInfo {
        ShardManagerInfo {
            host: self.host.clone(),
            grpc_port: self.grpc_port,
            http_port: self.http_port,
        }
    }
}

impl Drop for ShardManager<'_> {
    fn drop(&mut self) {
        match &mut self.inner {
            ShardManagerInner::Process(child) => {
                // TODO: SIGTERM, maybe with nix crate.
                match futures::executor::block_on(child.kill()) {
                    Ok(_) => println!("Killed Shard Manager process"),
                    Err(e) => eprintln!("Failed to kill Shard Manager process: {e:? }"),
                }
            }
            ShardManagerInner::Docker(_) => {}
            ShardManagerInner::K8S { .. } => {}
        }

        println!("Stopped Shard Manager")
    }
}

#[derive(Debug, Clone)]
pub struct ShardManagerInfo {
    pub host: String,
    pub http_port: u16,
    pub grpc_port: u16,
}
