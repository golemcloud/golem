use crate::context::db::DbInfo;
use crate::context::{EnvConfig, NETWORK, TAG};
use golem_cli::clients::template::{TemplateClient, TemplateClientLive};
use libtest_mimic::Failed;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use testcontainers::core::WaitFor;
use testcontainers::{clients, Container, Image, RunnableImage};
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
    local_http_port: u16,
    http_port: u16,
    grpc_port: u16,
    inner: GolemTemplateServiceInner<'docker_client>,
}

enum GolemTemplateServiceInner<'docker_client> {
    Process(Child),
    #[allow(dead_code)]
    Docker(Container<'docker_client, GolemTemplateServiceImage>),
}

impl<'docker_client> GolemTemplateService<'docker_client> {
    fn wait_for_health_check(http_port: u16) {
        let client = TemplateClientLive {
            client: golem_client::api::TemplateClientLive {
                context: golem_client::Context {
                    client: reqwest::Client::default(),
                    base_url: Url::parse(&format!("http://localhost:{http_port}")).unwrap(),
                },
            },
        };

        let wait_loop = async {
            loop {
                match client.find(None).await {
                    Ok(_) => break,
                    Err(e) => {
                        println!("Cloud Service healthcheck failed: {e:?}");
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
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

    pub fn start(
        docker: &'docker_client clients::Cli,
        env_config: &EnvConfig,
        db: &DbInfo,
    ) -> Result<GolemTemplateService<'docker_client>, Failed> {
        if env_config.local_golem {
            GolemTemplateService::start_process(env_config, db)
        } else {
            GolemTemplateService::start_docker(docker, env_config, db)
        }
    }

    fn start_docker(
        docker: &'docker_client clients::Cli,
        env_config: &EnvConfig,
        db: &DbInfo,
    ) -> Result<GolemTemplateService<'docker_client>, Failed> {
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

    fn start_process(
        env_config: &EnvConfig,
        db: &DbInfo,
    ) -> Result<GolemTemplateService<'docker_client>, Failed> {
        println!("Starting Golem Template Service");

        let service_dir = Path::new("../golem-template-service/");
        let service_run = Path::new("../target/debug/golem-template-service");

        if !service_run.exists() {
            return Err(format!(
                "Expected to have precompiled Golem Template Service at {}",
                service_run.to_str().unwrap_or("")
            )
            .into());
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
            .ok_or::<Failed>("Can't get golem template service stdout".into())?;

        // TODO: use tokio
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                if !quiet {
                    println!("[golemtemplate]   {}", line.unwrap())
                }
            }
        });

        let stderr = child
            .stderr
            .take()
            .ok_or::<Failed>("Can't get golem template service stderr".into())?;

        std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                if !quiet {
                    eprintln!("[golemtemplate]   {}", line.unwrap())
                }
            }
        });

        GolemTemplateService::wait_for_health_check(http_port);

        Ok(GolemTemplateService {
            host: "localhost".to_string(),
            local_http_port: http_port,
            http_port,
            grpc_port,
            inner: GolemTemplateServiceInner::Process(child),
        })
    }

    pub fn info(&self) -> GolemTemplateServiceInfo {
        GolemTemplateServiceInfo {
            host: self.host.clone(),
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
                match child.kill() {
                    Ok(_) => println!("Killed Golem Template Service process"),
                    Err(e) => eprintln!("Failed to kill Golem Template Service process: {e:? }"),
                }
            }
            GolemTemplateServiceInner::Docker(_) => {}
        }

        println!("Stopped Golem Template Service")
    }
}

#[derive(Debug)]
pub struct GolemTemplateServiceInfo {
    pub host: String,
    pub local_http_port: u16,
    pub http_port: u16,
    pub grpc_port: u16,
}
