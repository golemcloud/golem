use crate::context::{EnvConfig, NETWORK};
use libtest_mimic::Failed;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use testcontainers::{clients, Container, RunnableImage};

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
    #[allow(dead_code)]
    Docker(Container<'docker_client, testcontainers_modules::redis::Redis>),
    None,
}

impl<'docker_client> Redis<'docker_client> {
    fn check_if_redis_is_running(host: &str, port: u16) -> Result<(), Failed> {
        let client = ::redis::Client::open((host, port))?;
        let mut con = client.get_connection()?;
        ::redis::cmd("INFO").arg("server").query(&mut con)?;

        Ok(())
    }

    fn wait_started(port: u16) -> Result<(), Failed> {
        for _ in 0..10 {
            match Redis::check_if_redis_is_running("localhost", port) {
                Ok(_) => return Ok(()),
                Err(_) => {
                    println!("Can't find Redis on port {port}");
                    std::thread::sleep(Duration::from_secs(1))
                }
            }
        }

        Err("Failed to start Redis".into())
    }

    fn spawn_monitor(port: u16, env_config: &EnvConfig) -> Result<Child, Failed> {
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
            .ok_or::<Failed>("Can't get redis-cli stdout".into())?;

        let quiet = env_config.quiet;

        // TODO: use tokio
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                if !quiet {
                    println!("[redis-cli]   {}", line.unwrap())
                }
            }
        });

        let stderr = child
            .stderr
            .take()
            .ok_or::<Failed>("Can't get redis-cli stderr".into())?;

        std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                if !quiet {
                    eprintln!("[redis-cli]   {}", line.unwrap())
                }
            }
        });

        Ok(child)
    }

    fn start(port: u16, env_config: &EnvConfig) -> Result<Redis<'docker_client>, Failed> {
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
            .ok_or::<Failed>("Can't get Redis stdout".into())?;

        // TODO: use tokio
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                if !quiet {
                    println!("[redis]   {}", line.unwrap())
                }
            }
        });

        let stderr = child
            .stderr
            .take()
            .ok_or::<Failed>("Can't get Redis stderr".into())?;

        std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                if !quiet {
                    eprintln!("[redis]   {}", line.unwrap())
                }
            }
        });

        Redis::wait_started(port)?;

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

    fn make_process(env_config: &EnvConfig) -> Result<Redis<'docker_client>, Failed> {
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

        Redis::start(redis_port, env_config)
    }

    pub fn make(
        docker: &'docker_client clients::Cli,
        env_config: &EnvConfig,
    ) -> Result<Redis<'docker_client>, Failed> {
        if env_config.local_golem {
            Redis::make_process(env_config)
        } else {
            Redis::start_docker(docker)
        }
    }

    fn start_docker(docker: &'docker_client clients::Cli) -> Result<Redis<'docker_client>, Failed> {
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
                match monitor_child.kill() {
                    Ok(_) => println!("Killed redis-cli monitor process"),
                    Err(e) => eprintln!("Failed to kill redis-cli monitor process: {e:? }"),
                }

                match child.kill() {
                    Ok(_) => println!("Killed Redis process"),
                    Err(e) => eprintln!("Failed to kill Redis process: {e:? }"),
                }
            }
            RedisInner::Docker(_) => {}
            RedisInner::None => {}
        }

        println!("Stopped Redis")
    }
}

#[derive(Debug)]
pub struct RedisInfo {
    pub host: String,
    pub port: u16,
}
