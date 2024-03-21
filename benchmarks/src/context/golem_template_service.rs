use crate::context::db::DbInfo;
use crate::context::routing::Routing;
use crate::context::{
    EnvConfig, K8sNamespace, K8sRoutingType, ManagedPod, ManagedRouting, ManagedService, Runtime,
    NETWORK, TAG,
};
use anyhow::{anyhow, Result};
use golem_cli::clients::template::{TemplateClient, TemplateClientLive};
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
use tokio::time::Duration;
use url::Url;

#[derive(Debug)]
struct GolemTemplateServiceImage {
    grpc_port: u16,
    http_port: u16,
    env_vars: HashMap<String, String>,
}

impl GolemTemplateServiceImage {
    pub fn new(
        grpc_port: u16,
        http_port: u16,
        env_vars: HashMap<String, String>,
    ) -> GolemTemplateServiceImage {
        GolemTemplateServiceImage {
            grpc_port,
            http_port,
            env_vars,
        }
    }
}

impl Image for GolemTemplateServiceImage {
    type Args = ();

    fn name(&self) -> String {
        "golemservices/golem-template-service".to_string()
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
        vec![self.grpc_port, self.http_port]
    }
}

pub struct GolemTemplateService<'docker_client> {
    host: String,
    local_host: String,
    local_http_port: u16,
    http_port: u16,
    grpc_port: u16,
    inner: GolemTemplateServiceInner<'docker_client>,
}

enum GolemTemplateServiceInner<'docker_client> {
    Process(Child),
    Docker(Container<'docker_client, GolemTemplateServiceImage>),
    K8S {
        _pod: ManagedPod,
        _service: ManagedService,
        _routing: ManagedRouting,
    },
}

impl<'docker_client> GolemTemplateService<'docker_client> {
    async fn wait_for_health_check(schema: &str, host: &str, http_port: u16) {
        let client = TemplateClientLive {
            client: golem_client::api::TemplateClientLive {
                context: golem_client::Context {
                    client: reqwest::Client::default(),
                    base_url: Url::parse(&format!("{schema}://{host}:{http_port}")).unwrap(),
                },
            },
        };

        loop {
            match client.find(None).await {
                Ok(_) => break,
                Err(e) => {
                    println!("Cloud Service healthcheck failed: ${e:?}");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    }

    pub async fn start(
        docker: &'docker_client clients::Cli,
        env_config: &EnvConfig,
        db: &DbInfo,
    ) -> Result<GolemTemplateService<'docker_client>> {
        match &env_config.runtime {
            Runtime::Local => GolemTemplateService::start_process(env_config, db).await,
            Runtime::Docker => GolemTemplateService::start_docker(docker, env_config, db),
            Runtime::K8S { namespace, routing } => {
                GolemTemplateService::start_k8s(env_config, db, namespace, routing).await
            }
        }
    }

    async fn start_k8s(
        env_config: &EnvConfig,
        db: &DbInfo,
        namespace: &K8sNamespace,
        k8s_routing_type: &K8sRoutingType,
    ) -> Result<GolemTemplateService<'docker_client>> {
        println!("Starting Golem Template Service pod");

        let http_port = 8081;
        let grpc_port = 9091;

        let env_vars = GolemTemplateService::env_vars(grpc_port, http_port, env_config, db);
        let env_vars = env_vars
            .into_iter()
            .map(|(k, v)| json!({"name": k, "value": v}))
            .collect::<Vec<_>>();
        let name = "golem-template-service";

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
                    }
                ],
                "containers": [{
                    "name": "service",
                    "image": format!("golemservices/golem-template-service:{}", TAG),
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

        // println!("Started template service on {local_host}:{local_port}");
        //
        // GolemTemplateService::wait_for_health_check(&env_config.schema, &local_host, local_port).await;

        Ok(GolemTemplateService {
            host: format!("{name}.{}.svc.cluster.local", &namespace.0),
            local_host,
            local_http_port: local_port,
            http_port,
            grpc_port,
            inner: GolemTemplateServiceInner::K8S {
                _pod: managed_pod,
                _service: managed_service,
                _routing: routing,
            },
        })
    }

    fn start_docker(
        docker: &'docker_client clients::Cli,
        env_config: &EnvConfig,
        db: &DbInfo,
    ) -> Result<GolemTemplateService<'docker_client>> {
        println!("Starting Golem Template Service docker");

        let http_port = 8081;
        let grpc_port = 9091;

        let env_vars = GolemTemplateService::env_vars(grpc_port, http_port, env_config, db);
        let name = "golem_template_service";

        let image = RunnableImage::from(GolemTemplateServiceImage::new(
            grpc_port, http_port, env_vars,
        ))
        .with_container_name(name)
        .with_network(NETWORK);
        let node = docker.run(image);

        Ok(GolemTemplateService {
            host: name.to_string(),
            local_host: "localhost".to_string(),
            local_http_port: node.get_host_port_ipv4(http_port),
            http_port,
            grpc_port,
            inner: GolemTemplateServiceInner::Docker(node),
        })
    }

    fn env_vars(
        grpc_port: u16,
        http_port: u16,
        env_config: &EnvConfig,
        db: &DbInfo,
    ) -> HashMap<String, String> {
        let log_level = if env_config.verbose { "debug" } else { "info" };

        let vars: &[(&str, &str)] = &[
            ("RUST_LOG"                                   , &format!("{log_level},cranelift_codegen=warn,wasmtime_cranelift=warn,wasmtime_jit=warn,h2=warn,hyper=warn,tower=warn")),
            ("WASMTIME_BACKTRACE_DETAILS"                             , "1"),
            ("RUST_BACKTRACE"                             , "1"),
            ("GOLEM__WORKSPACE"                           , "ittest"),
            ("ENVIRONMENT"                         , "local"),
            ("GOLEM__TEMPLATE_STORE__TYPE", "Local"),
            ("GOLEM__TEMPLATE_STORE__CONFIG__OBJECT_PREFIX", ""),
            ("GOLEM__TEMPLATE_STORE__CONFIG__ROOT_PATH", "/tmp/ittest-local-object-store/golem"),
            ("GOLEM__GRPC_PORT", &grpc_port.to_string()),
            ("GOLEM__HTTP_PORT", &http_port.to_string()),

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
        db: &DbInfo,
    ) -> Result<GolemTemplateService<'docker_client>> {
        println!("Starting Golem Template Service");

        let service_dir = Path::new("../golem-template-service/");
        let service_run = Path::new("../target/debug/golem-template-service");

        if !service_run.exists() {
            return Err(anyhow!(
                "Expected to have precompiled Golem Template Service at {}",
                service_run.to_str().unwrap_or("")
            ));
        }

        println!("Starting {}", service_run.to_str().unwrap_or(""));

        let http_port = 8081;
        let grpc_port = 9091;

        let mut child = Command::new(service_run)
            .current_dir(service_dir)
            .envs(GolemTemplateService::env_vars(
                grpc_port, http_port, env_config, db,
            ))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let quiet = env_config.quiet;

        let stdout = child
            .stdout
            .take()
            .ok_or(anyhow!("Can't get golem template service stdout"))?;

        tokio::spawn(async move {
            let reader = BufReader::new(stdout);
            let mut lines = reader.lines();

            while let Some(line) = lines.next_line().await.unwrap() {
                if !quiet {
                    println!("[golemtemplate]   {}", line)
                }
            }
        });

        let stderr = child
            .stderr
            .take()
            .ok_or(anyhow!("Can't get golem template service stderr"))?;

        tokio::spawn(async move {
            let reader = BufReader::new(stderr);

            let mut lines = reader.lines();

            while let Some(line) = lines.next_line().await.unwrap() {
                if !quiet {
                    println!("[golemtemplate]   {}", line)
                }
            }
        });

        GolemTemplateService::wait_for_health_check(&env_config.schema, "localhost", http_port)
            .await;

        Ok(GolemTemplateService {
            host: "localhost".to_string(),
            local_host: "localhost".to_string(),
            local_http_port: http_port,
            http_port,
            grpc_port,
            inner: GolemTemplateServiceInner::Process(child),
        })
    }

    pub fn info(&self) -> GolemTemplateServiceInfo {
        GolemTemplateServiceInfo {
            host: self.host.clone(),
            local_host: self.local_host.clone(),
            local_http_port: self.local_http_port,
            http_port: self.http_port,
            grpc_port: self.grpc_port,
        }
    }
}

impl Drop for GolemTemplateService<'_> {
    fn drop(&mut self) {
        match &mut self.inner {
            GolemTemplateServiceInner::Process(child) => {
                // TODO: SIGTERM, maybe with nix crate.
                match futures::executor::block_on(child.kill()) {
                    Ok(_) => println!("Killed Golem Template Service process"),
                    Err(e) => eprintln!("Failed to kill Golem Template Service process: {e:? }"),
                }
            }
            GolemTemplateServiceInner::Docker(_) => {}
            GolemTemplateServiceInner::K8S { .. } => {}
        }

        println!("Stopped Golem Template Service")
    }
}

#[derive(Debug, Clone)]
pub struct GolemTemplateServiceInfo {
    pub host: String,
    pub local_host: String,
    pub local_http_port: u16,
    pub http_port: u16,
    pub grpc_port: u16,
}
