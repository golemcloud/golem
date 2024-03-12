use crate::context::db::{Db, DbInner};
use crate::context::{EnvConfig, ManagedPod, ManagedService, Runtime, NETWORK, K8sNamespace, K8sRoutingType};
use anyhow::{anyhow, Result};
use k8s_openapi::api::core::v1::{Pod, Service};
use kube::api::PostParams;
use kube::{Api, Client};
use serde_json::json;
use std::process::Stdio;
use std::time::Duration;
use testcontainers::{clients, Container, RunnableImage};
use tokio::io::AsyncBufReadExt;
use tokio::io::BufReader;
use tokio::process::{Child, Command};
use tokio::time::sleep;
use url::Url;
use crate::context::routing::Routing;

pub struct Redis<'docker_client> {
    host: String,
    port: u16,
    inner: RedisInner<'docker_client>,
}

enum RedisInner<'docker_client> {
    Process {
        child: Child,
        monitor_child: Child,
    },
    Docker(Container<'docker_client, testcontainers_modules::redis::Redis>),
    K8S {
        _pod: ManagedPod,
        _service: ManagedService,
    },
    None,
}

impl<'docker_client> Redis<'docker_client> {
    fn check_if_redis_is_running(host: &str, port: u16) -> Result<()> {
        let client = ::redis::Client::open((host, port))?;
        let mut con = client.get_connection()?;
        ::redis::cmd("INFO").arg("server").query(&mut con)?;

        Ok(())
    }

    async fn wait_started(host: &str, port: u16) -> Result<()> {
        for _ in 0..20 {
            match Redis::check_if_redis_is_running(host, port) {
                Ok(_) => return Ok(()),
                Err(e) => {
                    println!("Can't connect to Redis on {host}:{port}: {e:?}");
                    sleep(Duration::from_secs(1)).await
                }
            }
        }

        Err(anyhow!("Failed to start Redis"))
    }

    fn spawn_monitor(port: u16, env_config: &EnvConfig) -> Result<Child> {
        let mut child = Command::new("redis-cli")
            .arg("-p")
            .arg(port.to_string())
            .arg("monitor")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let stdout = child
            .stdout
            .take()
            .ok_or(anyhow!("Can't get redis-cli stdout"))?;

        let quiet = env_config.quiet;

        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();

            while let Some(line) = lines.next_line().await.unwrap() {
                if !quiet {
                    println!("[redis-cli]   {}", line)
                }
            }
        });

        let stderr = child
            .stderr
            .take()
            .ok_or(anyhow!("Can't get redis-cli stderr"))?;

        tokio::spawn(async move {
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();

            while let Some(line) = lines.next_line().await.unwrap() {
                if !quiet {
                    println!("[redis-cli]   {}", line)
                }
            }
        });

        Ok(child)
    }

    async fn start(port: u16, env_config: &EnvConfig) -> Result<Redis<'docker_client>> {
        println!("Starting redis on port {port}...");

        let mut child = Command::new("redis-server")
            .arg("--port")
            .arg(port.to_string())
            .arg("--save")
            .arg("")
            .arg("--appendonly")
            .arg("no")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let quiet = env_config.quiet;

        let stdout = child
            .stdout
            .take()
            .ok_or(anyhow!("Can't get Redis stdout"))?;

        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();

            while let Some(line) = lines.next_line().await.unwrap() {
                if !quiet {
                    println!("[redis]   {}", line)
                }
            }
        });

        let stderr = child
            .stderr
            .take()
            .ok_or(anyhow!("Can't get Redis stderr"))?;

        tokio::spawn(async move {
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();

            while let Some(line) = lines.next_line().await.unwrap() {
                if !quiet {
                    println!("[redis]   {}", line)
                }
            }
        });

        Redis::wait_started("localhost", port).await?;

        let monitor_child = Redis::spawn_monitor(port, env_config)?;

        Ok(Redis {
            host: "localhost".to_string(),
            port,
            inner: RedisInner::Process {
                child,
                monitor_child,
            },
        })
    }

    async fn make_process(env_config: &EnvConfig) -> Result<Redis<'docker_client>> {
        let default_redis_host = "localhost".to_string();
        let default_redis_port = 6379;

        let redis_host = std::env::var("REDIS_HOST").unwrap_or(default_redis_host);
        let redis_port: u16 = std::env::var("REDIS_PORT")
            .unwrap_or(default_redis_port.to_string())
            .parse()?;

        println!("Checking if Redis is already running on {redis_host}:{redis_port}");

        if Redis::check_if_redis_is_running(&redis_host, redis_port).is_ok() {
            println!("Using existing Redis");

            return Ok(Redis {
                host: redis_host,
                port: redis_port,
                inner: RedisInner::None,
            });
        }

        Redis::start(redis_port, env_config).await
    }

    pub async fn make(
        docker: &'docker_client clients::Cli,
        env_config: &EnvConfig,
    ) -> Result<Redis<'docker_client>> {
        match &env_config.runtime {
            Runtime::Local => Redis::make_process(env_config).await,
            Runtime::Docker => Redis::start_docker(docker),
            Runtime::K8S{namespace, routing: _ } => Redis::start_k8s(namespace).await,
        }
    }

    fn start_docker(docker: &'docker_client clients::Cli) -> Result<Redis<'docker_client>> {
        let name = "golem_redis";
        let image = RunnableImage::from(testcontainers_modules::redis::Redis)
            .with_tag("7.2")
            .with_container_name(name)
            .with_network(NETWORK);
        let node = docker.run(image);

        let res = Redis {
            host: name.to_string(),
            port: 6379,
            inner: RedisInner::Docker(node),
        };

        Ok(res)
    }

    async fn start_k8s(namespace: &K8sNamespace) -> Result<Redis<'docker_client>> {
        println!("Creating Redis pod");

        let pods: Api<Pod> = Api::namespaced(Client::try_default().await?, &namespace.0);
        let services: Api<Service> = Api::namespaced(Client::try_default().await?, &namespace.0);

        let pod: Pod = serde_json::from_value(json!({
            "apiVersion": "v1",
            "kind": "Pod",
            "metadata": {
                "name": "golem-redis",
                "labels": {
                    "app": "golem-redis",
                    "app-group": "golem"
                },
            },
            "spec": {
                "ports": [{
                    "port": 6379,
                    "protocol": "TCP"
                }],
                "containers": [{
                    "name": "redis",
                    "image": "redis:7.2"
                }]
            }
        }))?;

        let pp = PostParams::default();

        let res_pod = pods.create(&pp, &pod).await?;

        let managed_pod = ManagedPod::new("golem-redis", namespace);

        let service: Service = serde_json::from_value(json!({
            "apiVersion": "v1",
            "kind": "Service",
            "metadata": {
                "name": "golem-redis",
                "labels": {
                    "app": "golem-redis",
                    "app-group": "golem"
                },
            },
            "spec": {
                "ports": [{
                    "port": 6379,
                    "protocol": "TCP"
                }],
                "selector": { "app": "golem-redis" },
                "type": "LoadBalancer"
            }
        }))?;

        let res_srv = services.create(&pp, &service).await?;

        let managed_service = ManagedService::new("golem-redis", namespace);

        println!("Test Redis started");

        // Redis::wait_started(&local_host, local_port).await?;

        // println!("Redis started on host {local_host}, port {local_port}");

        let host = format!("golem-redis.{}.svc.cluster.local", &namespace.0);
        let port = 6379;

        Ok(Redis {
            host,
            port,
            inner: RedisInner::K8S {
                _pod: managed_pod,
                _service: managed_service,
            },
        })
    }

    pub fn info(&self) -> RedisInfo {
        RedisInfo {
            host: self.host.to_string(),
            port: self.port,
        }
    }
}

impl Drop for Redis<'_> {
    fn drop(&mut self) {
        match &mut self.inner {
            RedisInner::Process {
                child,
                monitor_child,
            } => {
                // TODO: SIGTERM, maybe with nix crate.
                match futures::executor::block_on(monitor_child.kill()) {
                    Ok(_) => println!("Killed redis-cli monitor process"),
                    Err(e) => eprintln!("Failed to kill redis-cli monitor process: {e:? }"),
                }

                match futures::executor::block_on(child.kill()) {
                    Ok(_) => println!("Killed Redis process"),
                    Err(e) => eprintln!("Failed to kill Redis process: {e:? }"),
                }
            }
            RedisInner::Docker(_) => {}
            RedisInner::K8S { .. } => {}
            RedisInner::None => {}
        }

        println!("Stopped Redis")
    }
}

#[derive(Debug, Clone)]
pub struct RedisInfo {
    pub host: String,
    pub port: u16,
}
