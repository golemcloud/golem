use golem_cli::clients::template::*;
use libtest_mimic::Failed;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use testcontainers::core::WaitFor;
use testcontainers::{clients, Container, Image, RunnableImage};
use tokio::time::timeout;
use tonic::transport::Uri;
use tonic_health::pb::health_check_response::ServingStatus;
use tonic_health::pb::health_client::HealthClient;
use tonic_health::pb::HealthCheckRequest;
use url::Url;

pub struct Postgres<'docker_client> {
    host: String,
    port: u16,
    local_port: u16,
    _node: Container<'docker_client, testcontainers_modules::postgres::Postgres>,
}

const NETWORK: &str = "golem_test_network";

impl<'docker_client> Postgres<'docker_client> {
    fn test_started(&self) -> Result<(), Failed> {
        let mut conn =
            postgres::Client::connect(&self.connection_string(), postgres::NoTls).unwrap();

        let _ = conn.query("SELECT version()", &[])?;
        Ok(())
    }

    pub fn start(
        docker: &'docker_client clients::Cli,
        env_config: &EnvConfig,
    ) -> Result<Postgres<'docker_client>, Failed> {
        let name = "golem_postgres";
        let image = RunnableImage::from(testcontainers_modules::postgres::Postgres::default())
            .with_tag("latest")
            .with_container_name(name)
            .with_network(NETWORK);
        let node = docker.run(image);

        let host = if env_config.local_golem {
            "localhost"
        } else {
            name
        };

        let port = if env_config.local_golem {
            node.get_host_port_ipv4(5432)
        } else {
            5432
        };

        let res = Postgres {
            host: host.to_string(),
            port,
            local_port: node.get_host_port_ipv4(5432),
            _node: node,
        };

        res.test_started()?;

        Ok(res)
    }

    pub fn connection_string(&self) -> String {
        format!(
            "postgres://postgres:postgres@127.0.0.1:{}/postgres",
            self.local_port,
        )
    }

    pub fn info(&self) -> PostgresInfo {
        PostgresInfo {
            host: self.host.clone(),
            port: self.port,
            local_port: self.local_port,
            database_name: "postgres".to_owned(),
            username: "postgres".to_owned(),
            password: "postgres".to_owned(),
        }
    }
}

pub struct Redis<'docker_client> {
    host: String,
    port: u16,
    inner: RedisInner<'docker_client>,
}

enum RedisInner<'docker_client> {
    Process { child: Child, monitor_child: Child },
    Docker(Container<'docker_client, testcontainers_modules::redis::Redis>),
    None,
}

impl<'docker_client> Redis<'docker_client> {
    fn check_if_redis_is_running(host: &str, port: u16) -> Result<(), Failed> {
        let client = redis::Client::open((host, port))?;
        let mut con = client.get_connection()?;
        redis::cmd("INFO").arg("server").query(&mut con)?;

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
            port: port,
            inner: RedisInner::Process {
                child: child,
                monitor_child: monitor_child,
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
        let image = RunnableImage::from(testcontainers_modules::redis::Redis::default())
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

#[derive(Debug, Clone)]
pub struct EnvConfig {
    pub verbose: bool,
    pub on_ci: bool,
    pub quiet: bool,
    pub redis_key_prefix: String,
    pub wasi_root: PathBuf,
    pub local_golem: bool,
}

impl EnvConfig {
    pub fn from_env() -> EnvConfig {
        EnvConfig {
            verbose: std::env::var("CI").is_err(),
            on_ci: std::env::var("CI").is_ok(),
            quiet: std::env::var("QUIET").is_ok(),
            redis_key_prefix: std::env::var("REDIS_KEY_PREFIX").unwrap_or("".to_string()),
            wasi_root: PathBuf::from("../test-templates"),
            local_golem: std::env::var("GOLEM_DOCKER_SERVICES").is_err(),
        }
    }
}

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

const TAG: &str = "v0.0.60";

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
            ("RUST_BACKTRACE"                             , "1"),
            ("GOLEM__WORKSPACE"                           , "ittest"),
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

pub struct WorkerExecutor<'docker_client> {
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
        golem: &GolemServiceInfo,
        shard_manager: &ShardManagerInfo,
    ) -> Result<WorkerExecutor<'docker_client>, Failed> {
        if env_config.local_golem {
            WorkerExecutor::start_process(shard_id, env_config, redis, golem, shard_manager)
        } else {
            WorkerExecutor::start_docker(docker, shard_id, env_config, redis, golem, shard_manager)
        }
    }

    fn start_docker(
        docker: &'docker_client clients::Cli,
        shard_id: u16,
        env_config: &EnvConfig,
        redis: &RedisInfo,
        golem: &GolemServiceInfo,
        shard_manager: &ShardManagerInfo,
    ) -> Result<WorkerExecutor<'docker_client>, Failed> {
        println!("Starting Worker Executor {shard_id} docker");

        let port = 9000 + shard_id;
        let http_port = 9100 + shard_id;

        let env_vars =
            WorkerExecutor::env_vars(port, http_port, env_config, redis, golem, shard_manager);
        let name = format!("golem_worker_executor{shard_id}");

        let image = RunnableImage::from(WorkerExecutorImage::new(port, http_port, env_vars))
            .with_container_name(&name)
            .with_network(NETWORK);
        let node = docker.run(image);

        Ok(WorkerExecutor {
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
        golem: &GolemServiceInfo,
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
            ("GOLEM__TEMPLATE_SERVICE__CONFIG__HOST"         , &golem.host),
            ("GOLEM__TEMPLATE_SERVICE__CONFIG__PORT"         , &golem.grpc_port.to_string()),
            ("GOLEM__TEMPLATE_SERVICE__CONFIG__ACCESS_TOKEN" , "2A354594-7A63-4091-A46B-CC58D379F677"),
            ("GOLEM__COMPILED_TEMPLATE_SERVICE__TYPE"        , "Disabled"),
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
        golem: &GolemServiceInfo,
        shard_manager: &ShardManagerInfo,
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
                golem,
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

        WorkerExecutor::wait_for_health_check(port);

        println!("Worker Executor {shard_id} online");

        Ok(WorkerExecutor {
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
    worker_executors: Vec<WorkerExecutor<'docker_client>>,
}

impl<'docker_client> WorkerExecutors<'docker_client> {
    pub fn start(
        docker: &'docker_client clients::Cli,
        env_config: &EnvConfig,
        redis: &RedisInfo,
        golem: &GolemServiceInfo,
        shard_manager: &ShardManagerInfo,
    ) -> Result<WorkerExecutors<'docker_client>, Failed> {
        let shards = 3;

        let mut worker_executors = Vec::with_capacity(shards);

        for shard_id in 0..shards {
            worker_executors.push(WorkerExecutor::start(
                docker,
                shard_id.try_into().unwrap(),
                env_config,
                redis,
                golem,
                shard_manager,
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

pub struct Context<'docker_client> {
    env: EnvConfig,
    postgres: Postgres<'docker_client>,
    redis: Redis<'docker_client>,
    shard_manager: ShardManager<'docker_client>,
    golem_service: GolemService<'docker_client>,
    worker_executors: WorkerExecutors<'docker_client>,
}

impl Context<'_> {
    pub fn start(docker: &clients::Cli) -> Result<Context, Failed> {
        let env_config = EnvConfig::from_env();
        let postgres = Postgres::start(docker, &env_config)?;
        let redis = Redis::make(docker, &env_config)?;
        let shard_manager = ShardManager::start(docker, &env_config, &redis.info())?;
        let golem_service =
            GolemService::start(docker, &env_config, &shard_manager.info(), &postgres.info())?;
        let worker_executors = WorkerExecutors::start(
            docker,
            &env_config,
            &redis.info(),
            &golem_service.info(),
            &shard_manager.info(),
        )?;

        Ok(Context {
            env: env_config,
            postgres,
            redis,
            shard_manager,
            golem_service,
            worker_executors,
        })
    }

    pub fn info(&self) -> ContextInfo {
        ContextInfo {
            env: self.env.clone(),
            db: Box::new(self.postgres.info()),
            redis: self.redis.info(),
            shard_manager: self.shard_manager.info(),
            golem_service: self.golem_service.info(),
            worker_executors: self.worker_executors.info(),
        }
    }
}

impl Drop for Context<'_> {
    fn drop(&mut self) {
        println!("Stopping Context")
    }
}

#[derive(Debug)]
pub struct PostgresInfo {
    pub host: String,
    pub port: u16,
    pub local_port: u16,
    pub database_name: String,
    pub username: String,
    pub password: String,
}

#[derive(Debug)]
pub struct RedisInfo {
    pub host: String,
    pub port: u16,
}

impl PostgresInfo {
    pub fn connection_string(&self) -> String {
        format!(
            "postgres://{}:{}@{}:{}/{}",
            self.username, self.password, self.host, self.port, self.database_name
        )
    }

    pub fn env(&self) -> HashMap<String, String> {
        HashMap::from([
            ("DB_HOST".to_string(), self.host.clone()),
            ("DB_PORT".to_string(), self.port.to_string()),
            ("DB_NAME".to_string(), self.database_name.clone()),
            ("DB_USERNAME".to_string(), self.username.clone()),
            ("DB_PASSWORD".to_string(), self.password.clone()),
            ("COMPONENT_REPOSITORY_TYPE".to_string(), "jdbc".to_string()),
            ("GOLEM__DB__TYPE".to_string(), "Postgres".to_string()),
            (
                "GOLEM__DB__CONFIG__MAX_CONNECTIONS".to_string(),
                "10".to_string(),
            ),
            ("GOLEM__DB__CONFIG__HOST".to_string(), self.host.clone()),
            ("GOLEM__DB__CONFIG__PORT".to_string(), self.port.to_string()),
            (
                "GOLEM__DB__CONFIG__DATABASE".to_string(),
                self.database_name.clone(),
            ),
            (
                "GOLEM__DB__CONFIG__USERNAME".to_string(),
                self.username.clone(),
            ),
            (
                "GOLEM__DB__CONFIG__PASSWORD".to_string(),
                self.password.clone(),
            ),
        ])
    }
}

pub trait DbInfo {
    fn env(&self) -> HashMap<String, String>;
}

impl DbInfo for PostgresInfo {
    fn env(&self) -> HashMap<String, String> {
        self.env()
    }
}

#[derive(Debug)]
pub struct GolemServiceInfo {
    pub host: String,
    pub local_http_port: u16,
    pub http_port: u16,
    pub grpc_port: u16,
}

#[derive(Debug)]
pub struct ShardManagerInfo {
    pub host: String,
    pub http_port: u16,
    pub grpc_port: u16,
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

pub struct ContextInfo {
    pub env: EnvConfig,
    pub db: Box<dyn DbInfo + Sync + Send>,
    pub redis: RedisInfo,
    pub shard_manager: ShardManagerInfo,
    pub golem_service: GolemServiceInfo,
    pub worker_executors: WorkerExecutorsInfo,
}
