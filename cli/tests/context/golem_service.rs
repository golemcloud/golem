use crate::context::db::DbInfo;
use crate::context::{EnvConfig, ShardManagerInfo, NETWORK, TAG};
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
struct GolemServiceImage {
    grpc_port: u16,
    http_port: u16,
    env_vars: HashMap<String, String>,
}

impl GolemServiceImage {
    pub fn new(
        grpc_port: u16,
        http_port: u16,
        env_vars: HashMap<String, String>,
    ) -> GolemServiceImage {
        GolemServiceImage {
            grpc_port,
            http_port,
            env_vars,
        }
    }
}

impl Image for GolemServiceImage {
    type Args = ();

    fn name(&self) -> String {
        "golemservices/golem-service".to_string()
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

pub struct GolemService<'docker_client> {
    host: String,
    local_http_port: u16,
    http_port: u16,
    grpc_port: u16,
    inner: GolemServiceInner<'docker_client>,
}

enum GolemServiceInner<'docker_client> {
    Process(Child),
    Docker(Container<'docker_client, GolemServiceImage>),
}

impl<'docker_client> GolemService<'docker_client> {
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
                        println!("Cloud Service healthcheck failed: ${e:?}");
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
        shard_manager: &ShardManagerInfo,
        db: &impl DbInfo,
    ) -> Result<GolemService<'docker_client>, Failed> {
        if env_config.local_golem {
            GolemService::start_process(env_config, shard_manager, db)
        } else {
            GolemService::start_docker(docker, env_config, shard_manager, db)
        }
    }

    fn start_docker(
        docker: &'docker_client clients::Cli,
        env_config: &EnvConfig,
        shard_manager: &ShardManagerInfo,
        db: &impl DbInfo,
    ) -> Result<GolemService<'docker_client>, Failed> {
        println!("Starting Golem Service docker with shard manager: {shard_manager:?}");

        let http_port = 8081;
        let grpc_port = 9091;

        let env_vars = GolemService::env_vars(grpc_port, http_port, env_config, shard_manager, db);
        let name = "golem_service";

        let image = RunnableImage::from(GolemServiceImage::new(grpc_port, http_port, env_vars))
            .with_container_name(name)
            .with_network(NETWORK);
        let node = docker.run(image);

        Ok(GolemService {
            host: name.to_string(),
            local_http_port: node.get_host_port_ipv4(http_port),
            http_port,
            grpc_port,
            inner: GolemServiceInner::Docker(node),
        })
    }

    fn env_vars(
        grpc_port: u16,
        http_port: u16,
        env_config: &EnvConfig,
        shard_manager: &ShardManagerInfo,
        db: &impl DbInfo,
    ) -> HashMap<String, String> {
        let log_level = if env_config.verbose { "debug" } else { "info" };

        let vars: &[(&str, &str)] = &[
            ("RUST_LOG"                                   , &format!("{log_level},cranelift_codegen=warn,wasmtime_cranelift=warn,wasmtime_jit=warn,h2=warn,hyper=warn,tower=warn")),
            ("WASMTIME_BACKTRACE_DETAILS"                             , "1"),
            ("RUST_BACKTRACE"                             , "1"),
            ("GOLEM__WORKSPACE"                           , "ittest"),
            ("ENVIRONMENT"                         , "local"),
            ("GOLEM__ENVIRONMENT"                         , "ittest"),
            ("GOLEM__ROUTING_TABLE__HOST"                 , &shard_manager.host),
            ("GOLEM__ROUTING_TABLE__PORT"                 , &shard_manager.grpc_port.to_string()),
            ("GOLEM__ACCOUNTS__ROOT__TOKEN"               , "0868571c-b6cc-4817-bae8-048cbcef91a0"),
            ("GOLEM__ACCOUNTS__ROOT__ID"                  , "test-account"),
            ("GOLEM__ACCOUNTS__MARKETING__TOKEN"          , "7868571c-b6cc-4817-bae8-048cbcef91a0"),
            ("GOLEM__ED_DSA__PRIVATE_KEY"                 , "MC4CAQAwBQYDK2VwBCIEIDSMbnKaWIjXt+LQaA8pVudt6KBwVBaBecRPWZRvmwUW"),
            ("GOLEM__ED_DSA__PUBLIC_KEY"                  , "MCowBQYDK2VwAyEAVLrTJ1Q5Cl438J8/bQ1Ib+zfvb9VrOQjwzxKgVRoY7w="),
            ("GOLEM__TEMPLATES__STORE__CONFIG__ROOT_PATH" , "/tmp/ittest-local-object-store/golem"),
            ("GOLEM__TEMPLATES__STORE__TYPE", "Local"),
            ("GOLEM__TEMPLATES__STORE__CONFIG__OBJECT_PREFIX", ""),
            ("GOLEM__TEMPLATES__STORE__CONFIG__ROOT_PATH", "../target/template_store"),
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
        shard_manager: &ShardManagerInfo,
        db: &impl DbInfo,
    ) -> Result<GolemService<'docker_client>, Failed> {
        println!("Starting Golem Service");

        let service_dir = Path::new("../golem-service/");
        let service_run = Path::new("../target/debug/golem-service");

        if !service_run.exists() {
            return Err(format!(
                "Expected to have precompiled Cloud Service at {}",
                service_run.to_str().unwrap_or("")
            )
            .into());
        }

        println!("Starting {}", service_run.to_str().unwrap_or(""));

        let http_port = 8081;
        let grpc_port = 9091;

        let mut child = Command::new(service_run)
            .current_dir(service_dir)
            .envs(GolemService::env_vars(
                grpc_port,
                http_port,
                env_config,
                shard_manager,
                db,
            ))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let quiet = env_config.quiet;

        let stdout = child
            .stdout
            .take()
            .ok_or::<Failed>("Can't get golem service stdout".into())?;

        // TODO: use tokio
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                if !quiet {
                    println!("[golemsvc]   {}", line.unwrap())
                }
            }
        });

        let stderr = child
            .stderr
            .take()
            .ok_or::<Failed>("Can't get golem service stderr".into())?;

        std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                if !quiet {
                    eprintln!("[golemsvc]   {}", line.unwrap())
                }
            }
        });

        GolemService::wait_for_health_check(http_port);

        Ok(GolemService {
            host: "localhost".to_string(),
            local_http_port: http_port,
            http_port,
            grpc_port,
            inner: GolemServiceInner::Process(child),
        })
    }

    pub fn info(&self) -> GolemServiceInfo {
        GolemServiceInfo {
            host: self.host.clone(),
            local_http_port: self.local_http_port,
            http_port: self.http_port,
            grpc_port: self.grpc_port,
        }
    }
}

impl Drop for GolemService<'_> {
    fn drop(&mut self) {
        match &mut self.inner {
            GolemServiceInner::Process(child) => {
                // TODO: SIGTERM, maybe with nix crate.
                match child.kill() {
                    Ok(_) => println!("Killed Golem Service process"),
                    Err(e) => eprintln!("Failed to kill Golem Service process: {e:? }"),
                }
            }
            GolemServiceInner::Docker(_) => {}
        }

        println!("Stopped Golem Service")
    }
}

#[derive(Debug)]
pub struct GolemServiceInfo {
    pub host: String,
    pub local_http_port: u16,
    pub http_port: u16,
    pub grpc_port: u16,
}
