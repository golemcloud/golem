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
use golem_cli::clients::worker::{WorkerClient, WorkerClientLive};
use crate::context::golem_template_service::GolemTemplateServiceInfo;
use crate::context::redis::{Redis, RedisInfo};

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
    local_http_port: u16,
    http_port: u16,
    grpc_port: u16,
    custom_request_port: u16,
    inner: GolemWorkerServiceInner<'docker_client>,
}

enum GolemWorkerServiceInner<'docker_client> {
    Process(Child),
    Docker(Container<'docker_client, GolemWorkerServiceImage>),
}

impl<'docker_client> GolemWorkerService<'docker_client> {
    pub fn start(
        docker: &'docker_client clients::Cli,
        env_config: &EnvConfig,
        shard_manager: &ShardManagerInfo,
        db: &DbInfo,
        redis: &RedisInfo,
        template: &GolemTemplateServiceInfo,
    ) -> Result<GolemWorkerService<'docker_client>, Failed> {
        if env_config.local_golem {
            GolemWorkerService::start_process(env_config, shard_manager, db, redis, template)
        } else {
            GolemWorkerService::start_docker(docker, env_config, shard_manager, db, redis, template)
        }
    }

    fn start_docker(
        docker: &'docker_client clients::Cli,
        env_config: &EnvConfig,
        shard_manager: &ShardManagerInfo,
        db: &DbInfo,
        redis: &RedisInfo,
        template: &GolemTemplateServiceInfo,
    ) -> Result<GolemWorkerService<'docker_client>, Failed> {
        println!("Starting Golem Worker Service docker with shard manager: {shard_manager:?}");

        let http_port = 8082;
        let grpc_port = 9092;
        let custom_request_port = 9093;

        let env_vars = GolemWorkerService::env_vars(grpc_port, http_port, custom_request_port, env_config, shard_manager, db, redis, template);
        let name = "golem_worker_service";

        let image = RunnableImage::from(GolemWorkerServiceImage::new(grpc_port, http_port, custom_request_port, env_vars))
            .with_container_name(name)
            .with_network(NETWORK);
        let node = docker.run(image);

        Ok(GolemWorkerService {
            host: name.to_string(),
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

    fn start_process(
        env_config: &EnvConfig,
        shard_manager: &ShardManagerInfo,
        db: &DbInfo,
        redis: &RedisInfo,
        template: &GolemTemplateServiceInfo,
    ) -> Result<GolemWorkerService<'docker_client>, Failed> {
        println!("Starting Golem Worker Service");

        let service_dir = Path::new("../golem-worker-service/");
        let service_run = Path::new("../target/debug/golem-worker-service");

        if !service_run.exists() {
            return Err(format!(
                "Expected to have precompiled Golem Worker Service at {}",
                service_run.to_str().unwrap_or("")
            )
            .into());
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
                template
            ))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let quiet = env_config.quiet;

        let stdout = child
            .stdout
            .take()
            .ok_or::<Failed>("Can't get golem worker service stdout".into())?;

        // TODO: use tokio
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                if !quiet {
                    println!("[golemworker]   {}", line.unwrap())
                }
            }
        });

        let stderr = child
            .stderr
            .take()
            .ok_or::<Failed>("Can't get golem worker service stderr".into())?;

        std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                if !quiet {
                    eprintln!("[golemworker]   {}", line.unwrap())
                }
            }
        });

        // TODO; Wait for healthcheck

        Ok(GolemWorkerService {
            host: "localhost".to_string(),
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
            local_http_port: self.local_http_port,
            http_port: self.http_port,
            grpc_port: self.grpc_port,
            custom_request_port: self.custom_request_port
        }
    }
}

impl Drop for GolemWorkerService<'_> {
    fn drop(&mut self) {
        match &mut self.inner {
            GolemWorkerServiceInner::Process(child) => {
                // TODO: SIGTERM, maybe with nix crate.
                match child.kill() {
                    Ok(_) => println!("Killed Golem Worker Service process"),
                    Err(e) => eprintln!("Failed to kill Golem Worker Service process: {e:? }"),
                }
            }
            GolemWorkerServiceInner::Docker(_) => {}
        }

        println!("Stopped Golem Worker Service")
    }
}

#[derive(Debug)]
pub struct GolemWorkerServiceInfo {
    pub host: String,
    pub local_http_port: u16,
    pub http_port: u16,
    pub grpc_port: u16,
    pub custom_request_port: u16
}
