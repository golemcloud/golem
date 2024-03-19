use crate::context::redis::RedisInfo;
use crate::context::{EnvConfig, NETWORK, TAG};
use libtest_mimic::Failed;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use testcontainers::core::WaitFor;
use testcontainers::{clients, Container, Image, RunnableImage};

pub struct ShardManager<'docker_client> {
    host: String,
    http_port: u16,
    grpc_port: u16,
    inner: ShardManagerInner<'docker_client>,
}

enum ShardManagerInner<'docker_client> {
    Process(Child),
    Docker(Container<'docker_client, ShardManagerImage>),
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
    pub fn start(
        docker: &'docker_client clients::Cli,
        env_config: &EnvConfig,
        redis: &RedisInfo,
    ) -> Result<ShardManager<'docker_client>, Failed> {
        if env_config.local_golem {
            ShardManager::start_process(env_config, redis)
        } else {
            ShardManager::start_docker(docker, env_config, redis)
        }
    }

    fn start_docker(
        docker: &'docker_client clients::Cli,
        env_config: &EnvConfig,
        redis: &RedisInfo,
    ) -> Result<ShardManager<'docker_client>, Failed> {
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

    fn start_process(
        env_config: &EnvConfig,
        redis: &RedisInfo,
    ) -> Result<ShardManager<'docker_client>, Failed> {
        let port = 9020;
        let http_port = 9021;
        println!("Starting Golem Shard Manager");

        let service_dir = Path::new("../golem-shard-manager/");
        let service_run = Path::new("../target/debug/golem-shard-manager");

        if !service_run.exists() {
            return Err(format!(
                "Expected to have precompiled Cloud Shard Manager at {}",
                service_run.to_str().unwrap_or("")
            )
            .into());
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
            .ok_or::<Failed>("Can't get shard service stdout".into())?;

        // TODO: use tokio
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                if !quiet {
                    println!("[shardsvc]   {}", line.unwrap())
                }
            }
        });

        let stderr = child
            .stderr
            .take()
            .ok_or::<Failed>("Can't get shard service stderr".into())?;

        std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                if !quiet {
                    eprintln!("[shardsvc]   {}", line.unwrap())
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
                match child.kill() {
                    Ok(_) => println!("Killed Shard Manager process"),
                    Err(e) => eprintln!("Failed to kill Shard Manager process: {e:? }"),
                }
            }
            ShardManagerInner::Docker(_) => {}
        }

        println!("Stopped Shard Manager")
    }
}

#[derive(Debug)]
pub struct ShardManagerInfo {
    pub host: String,
    pub http_port: u16,
    pub grpc_port: u16,
}
