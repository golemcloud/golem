use crate::context::db::DbInfo;
use crate::context::golem_template_service::{GolemTemplateService, GolemTemplateServiceInfo};
use crate::context::redis::RedisInfo;
use crate::context::routing::Routing;
use crate::context::{
    EnvConfig, K8sNamespace, K8sRoutingType, ManagedPod, ManagedRouting, ManagedService, Runtime,
    ShardManagerInfo, NETWORK, TAG,
};
use anyhow::{anyhow, Result};
use k8s_openapi::api::core::v1::{Pod, Service};
use kube::api::PostParams;
use kube::{Api, Client};
use serde_json::json;
use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use testcontainers::core::WaitFor;
use testcontainers::{clients, Container, Image, RunnableImage};
use tokio::io::AsyncBufReadExt;
use tokio::io::BufReader;
use tokio::process::{Child, Command};
use url::Url;

#[derive(Debug)]
struct GolemWorkerServiceImage {
    grpc_port: u16,
    http_port: u16,
    custom_request_port: u16,
    env_vars: HashMap<String, String>,
}

impl GolemWorkerServiceImage {
    pub fn new(
        grpc_port: u16,
        http_port: u16,
        custom_request_port: u16,
        env_vars: HashMap<String, String>,
    ) -> GolemWorkerServiceImage {
        GolemWorkerServiceImage {
            grpc_port,
            http_port,
            custom_request_port,
            env_vars,
        }
    }
}

impl Image for GolemWorkerServiceImage {
    type Args = ();

    fn name(&self) -> String {
        "golemservices/golem-worker-service".to_string()
    }

    fn tag(&self) -> String {
        TAG.to_owned()
    }

    fn ready_conditions(&self) -> Vec<WaitFor> {
        vec![WaitFor::message_on_stdout("server started")]
    }

    fn env_vars(&self) -> Box<dyn Iterator<Item = (&String, &String)> + '_> {
        Box::new(self.env_vars.iter())
    }

    fn expose_ports(&self) -> Vec<u16> {
        vec![self.grpc_port, self.http_port, self.custom_request_port]
    }
}

pub struct GolemWorkerService<'docker_client> {
    host: String,
    local_host: String,
    local_http_port: u16,
    http_port: u16,
    grpc_port: u16,
    custom_request_port: u16,
    inner: GolemWorkerServiceInner<'docker_client>,
}

enum GolemWorkerServiceInner<'docker_client> {
    Process(Child),
    Docker(Container<'docker_client, GolemWorkerServiceImage>),
    K8S {
        _pod: ManagedPod,
        _service: ManagedService,
        _routing: ManagedRouting,
    },
}

impl<'docker_client> GolemWorkerService<'docker_client> {
    pub async fn start(
        docker: &'docker_client clients::Cli,
        env_config: &EnvConfig,
        shard_manager: &ShardManagerInfo,
        db: &DbInfo,
        redis: &RedisInfo,
        template: &GolemTemplateServiceInfo,
    ) -> Result<GolemWorkerService<'docker_client>> {
        match &env_config.runtime {
            Runtime::Local => {
                GolemWorkerService::start_process(env_config, shard_manager, db, redis, template)
                    .await
            }
            Runtime::Docker => GolemWorkerService::start_docker(
                docker,
                env_config,
                shard_manager,
                db,
                redis,
                template,
            ),
            Runtime::K8S { namespace, routing } => {
                GolemWorkerService::start_k8s(
                    env_config,
                    shard_manager,
                    db,
                    redis,
                    template,
                    namespace,
                    routing,
                )
                .await
            }
        }
    }

    async fn start_k8s(
        env_config: &EnvConfig,
        shard_manager: &ShardManagerInfo,
        db: &DbInfo,
        redis: &RedisInfo,
        template: &GolemTemplateServiceInfo,
        namespace: &K8sNamespace,
        k8s_routing_type: &K8sRoutingType,
    ) -> Result<GolemWorkerService<'docker_client>> {
        println!("Starting Golem Worker Service pod with shard manager: {shard_manager:?}");

        let http_port = 8082;
        let grpc_port = 9092;
        let custom_request_port = 9093;

        let env_vars = GolemWorkerService::env_vars(
            grpc_port,
            http_port,
            custom_request_port,
            env_config,
            shard_manager,
            db,
            redis,
            template,
        );
        let env_vars = env_vars
            .into_iter()
            .map(|(k, v)| json!({"name": k, "value": v}))
            .collect::<Vec<_>>();
        let name = "golem-worker-service";

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
                        "port": grpc_port,
                        "protocol": "TCP"
                    },
                    {
                        "port": http_port,
                        "protocol": "TCP"
                    },
                    {
                        "port": custom_request_port,
                        "protocol": "TCP"
                    }
                ],
                "containers": [{
                    "name": "service",
                    "image": format!("golemservices/golem-worker-service:{}", TAG),
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
                        "port": grpc_port,
                        "protocol": "TCP"
                    },
                    {
                        "name": "http",
                        "port": http_port,
                        "protocol": "TCP"
                    },
                    {
                        "name": "custom-request",
                        "port": custom_request_port,
                        "protocol": "TCP"
                    }
                ],
                "selector": { "app": name },
                "type": "LoadBalancer"
            }
        }))?;

        let res_srv = services.create(&pp, &service).await?;

        let managed_service = ManagedService::new(name, namespace);

        let Routing {
            hostname: local_host,
            port: local_port,
            routing,
        } = Routing::create(name, http_port, namespace, k8s_routing_type).await?;

        let local_port = match k8s_routing_type {
            K8sRoutingType::Minikube => res_srv
                .spec
                .as_ref()
                .ok_or(anyhow!("No spec in service"))?
                .ports
                .as_ref()
                .ok_or(anyhow!("No ports in service"))?
                .iter()
                .find(|p| p.name.iter().any(|n| n == "http"))
                .as_ref()
                .ok_or(anyhow!("No http port in spec"))?
                .node_port
                .ok_or(anyhow!("No node port for http port"))?
                as u16,
            K8sRoutingType::Ingress => local_port,
        };

        println!("Started template service on {local_host}:{local_port}");

        Ok(GolemWorkerService {
            host: format!("{name}.{}.svc.cluster.local", &namespace.0),
            local_host,
            local_http_port: local_port,
            http_port,
            grpc_port,
            custom_request_port,
            inner: GolemWorkerServiceInner::K8S {
                _pod: managed_pod,
                _service: managed_service,
                _routing: routing,
            },
        })
    }

    fn start_docker(
        docker: &'docker_client clients::Cli,
        env_config: &EnvConfig,
        shard_manager: &ShardManagerInfo,
        db: &DbInfo,
        redis: &RedisInfo,
        template: &GolemTemplateServiceInfo,
    ) -> Result<GolemWorkerService<'docker_client>> {
        println!("Starting Golem Worker Service docker with shard manager: {shard_manager:?}");

        let http_port = 8082;
        let grpc_port = 9092;
        let custom_request_port = 9093;

        let env_vars = GolemWorkerService::env_vars(
            grpc_port,
            http_port,
            custom_request_port,
            env_config,
            shard_manager,
            db,
            redis,
            template,
        );
        let name = "golem_worker_service";

        let image = RunnableImage::from(GolemWorkerServiceImage::new(
            grpc_port,
            http_port,
            custom_request_port,
            env_vars,
        ))
        .with_container_name(name)
        .with_network(NETWORK);
        let node = docker.run(image);

        Ok(GolemWorkerService {
            host: name.to_string(),
            local_host: "localhost".to_string(),
            local_http_port: node.get_host_port_ipv4(http_port),
            http_port,
            grpc_port,
            custom_request_port,
            inner: GolemWorkerServiceInner::Docker(node),
        })
    }

    fn env_vars(
        grpc_port: u16,
        http_port: u16,
        custom_request_port: u16,
        env_config: &EnvConfig,
        shard_manager: &ShardManagerInfo,
        db: &DbInfo,
        redis: &RedisInfo,
        template: &GolemTemplateServiceInfo,
    ) -> HashMap<String, String> {
        let log_level = if env_config.verbose { "debug" } else { "info" };

        let vars: &[(&str, &str)] = &[
            ("RUST_LOG"                                   , &format!("{log_level},cranelift_codegen=warn,wasmtime_cranelift=warn,wasmtime_jit=warn,h2=warn,hyper=warn,tower=warn")),
            ("WASMTIME_BACKTRACE_DETAILS"                             , "1"),
            ("RUST_BACKTRACE"                             , "1"),
            ("GOLEM__WORKSPACE"                           , "ittest"),
            ("GOLEM__REDIS__HOST"                         , &redis.host),
            ("GOLEM__REDIS__PORT"                        , &redis.port.to_string()),
            ("GOLEM__REDIS__DATABASE"                     , "1"),
            ("GOLEM__TEMPLATE_SERVICE__HOST"              , &template.host),
            ("GOLEM__TEMPLATE_SERVICE__PORT"              , &template.grpc_port.to_string()),
            ("GOLEM__TEMPLATE_SERVICE__ACCESS_TOKEN"      , "5C832D93-FF85-4A8F-9803-513950FDFDB1"),
            ("ENVIRONMENT"                                , "local"),
            ("GOLEM__ENVIRONMENT"                         , "ittest"),
            ("GOLEM__ROUTING_TABLE__HOST"                 , &shard_manager.host),
            ("GOLEM__ROUTING_TABLE__PORT"                 , &shard_manager.grpc_port.to_string()),
            ("GOLEM__CUSTOM_REQUEST_PORT"                 , &custom_request_port.to_string()),
            ("GOLEM__WORKER_GRPC_PORT", &grpc_port.to_string()),
            ("GOLEM__PORT", &http_port.to_string()),

        ];

        let mut vars: HashMap<String, String> = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();

        for (k, v) in db.env().into_iter() {
            vars.insert(k, v);
        }

        vars
    }

    async fn start_process(
        env_config: &EnvConfig,
        shard_manager: &ShardManagerInfo,
        db: &DbInfo,
        redis: &RedisInfo,
        template: &GolemTemplateServiceInfo,
    ) -> Result<GolemWorkerService<'docker_client>> {
        println!("Starting Golem Worker Service");

        let service_dir = Path::new("../golem-worker-service/");
        let service_run = Path::new("../target/debug/golem-worker-service");

        if !service_run.exists() {
            return Err(anyhow!(
                "Expected to have precompiled Golem Worker Service at {}",
                service_run.to_str().unwrap_or("")
            ));
        }

        println!("Starting {}", service_run.to_str().unwrap_or(""));

        let http_port = 8082;
        let grpc_port = 9092;
        let custom_request_port = 9093;

        let mut child = Command::new(service_run)
            .current_dir(service_dir)
            .envs(GolemWorkerService::env_vars(
                grpc_port,
                http_port,
                custom_request_port,
                env_config,
                shard_manager,
                db,
                redis,
                template,
            ))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let quiet = env_config.quiet;

        let stdout = child
            .stdout
            .take()
            .ok_or(anyhow!("Can't get golem worker service stdout"))?;

        // TODO: use tokio
        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();

            while let Some(line) = lines.next_line().await.unwrap() {
                if !quiet {
                    println!("[golemworker]   {}", line)
                }
            }
        });

        let stderr = child
            .stderr
            .take()
            .ok_or(anyhow!("Can't get golem worker service stderr"))?;

        tokio::spawn(async move {
            let reader = BufReader::new(stderr);
            let mut lines = reader.lines();

            while let Some(line) = lines.next_line().await.unwrap() {
                if !quiet {
                    println!("[golemworker]   {}", line)
                }
            }
        });

        // TODO; Wait for healthcheck

        Ok(GolemWorkerService {
            host: "localhost".to_string(),
            local_host: "localhost".to_string(),
            local_http_port: http_port,
            http_port,
            grpc_port,
            custom_request_port,
            inner: GolemWorkerServiceInner::Process(child),
        })
    }

    pub fn info(&self) -> GolemWorkerServiceInfo {
        GolemWorkerServiceInfo {
            host: self.host.clone(),
            local_host: self.local_host.clone(),
            local_http_port: self.local_http_port,
            http_port: self.http_port,
            grpc_port: self.grpc_port,
            custom_request_port: self.custom_request_port,
        }
    }
}

impl Drop for GolemWorkerService<'_> {
    fn drop(&mut self) {
        match &mut self.inner {
            GolemWorkerServiceInner::Process(child) => {
                // TODO: SIGTERM, maybe with nix crate.
                match futures::executor::block_on(child.kill()) {
                    Ok(_) => println!("Killed Golem Worker Service process"),
                    Err(e) => eprintln!("Failed to kill Golem Worker Service process: {e:? }"),
                }
            }
            GolemWorkerServiceInner::Docker(_) => {}
            GolemWorkerServiceInner::K8S { .. } => {}
        }

        println!("Stopped Golem Worker Service")
    }
}

#[derive(Debug, Clone)]
pub struct GolemWorkerServiceInfo {
    pub host: String,
    pub local_host: String,
    pub local_http_port: u16,
    pub http_port: u16,
    pub grpc_port: u16,
    pub custom_request_port: u16,
}
