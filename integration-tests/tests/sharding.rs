use std::ops::Deref;
use std::time::Duration;

use ctor::{ctor, dtor};
use golem_wasm_rpc::Value;
use rand::prelude::*;
use tokio::sync::oneshot::Receiver;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

use golem_api_grpc::proto::golem::worker;
use golem_common::model::WorkerId;
use golem_test_framework::config::{EnvBasedTestDependencies, TestDependencies};
use golem_test_framework::dsl::TestDsl;

struct Tracing;

impl Tracing {
    pub fn init() -> Self {
        // let console_layer = console_subscriber::spawn().with_filter(
        //     EnvFilter::try_new("trace").unwrap()
        //);
        let ansi_layer = tracing_subscriber::fmt::layer()
            .with_ansi(true)
            .with_filter(
                EnvFilter::try_new("debug,cranelift_codegen=warn,wasmtime_cranelift=warn,wasmtime_jit=warn,h2=warn,hyper=warn,tower=warn,fred=warn").unwrap()
            );

        tracing_subscriber::registry()
            // .with(console_layer) // Uncomment this to use tokio-console. Also needs RUSTFLAGS="--cfg tokio_unstable"
            .with(ansi_layer)
            .init();

        Self
    }
}

#[ctor]
pub static DEPS: EnvBasedTestDependencies = {
    let deps = EnvBasedTestDependencies::new(10);

    deps.redis_monitor().assert_valid();
    println!(
        "Started a cluster of {} worker executors",
        deps.worker_executor_cluster().size()
    );

    deps
};

#[dtor]
unsafe fn drop_deps() {
    let base_deps_ptr = DEPS.deref() as *const EnvBasedTestDependencies;
    let base_deps_ptr = base_deps_ptr as *mut EnvBasedTestDependencies;
    (*base_deps_ptr).kill_all()
}

#[ctor]
pub static TRACING: Tracing = Tracing::init();

enum Command {
    StartShard,
    StopShard,
    RestartShardManager,
}

fn start_shard() {
    let mut stopped = DEPS.worker_executor_cluster().stopped_indices();
    if !stopped.is_empty() {
        let mut rng = thread_rng();
        stopped.shuffle(&mut rng);

        DEPS.worker_executor_cluster().start(stopped[0]);
    }
}

fn stop_shard() {
    let mut started = DEPS.worker_executor_cluster().started_indices();
    if !started.is_empty() {
        let mut rng = thread_rng();
        started.shuffle(&mut rng);

        DEPS.worker_executor_cluster().stop(started[0]);
    }
}

fn reload_shard_manager() {
    DEPS.shard_manager().kill();
    DEPS.shard_manager().restart();
}

async fn make_env_unstable(mut stop_rx: Receiver<()>) {
    println!("!!! Starting Golem Sharding Tester");

    fn worker() {
        let mut commands = [
            Command::StartShard,
            Command::StopShard,
            Command::RestartShardManager,
        ];
        let mut rng = thread_rng();
        commands.shuffle(&mut rng);
        match commands[0] {
            Command::StartShard => {
                println!("!!! Golem Sharding Tester starting shard");
                start_shard();
                println!("!!! Golem Sharding Tester started shard");
            }
            Command::StopShard => {
                println!("!!! Golem Sharding Tester stopping shard");
                stop_shard();
                println!("!!! Golem Sharding Tester stopped shard");
            }
            Command::RestartShardManager => {
                println!("!!! Golem Sharding Tester reloading shard manager");
                reload_shard_manager();
                println!("!!! Golem Sharding Tester reloaded shard manager");
            }
        }
    }

    fn random_seconds() -> u64 {
        let mut rng = thread_rng();
        rng.gen_range(1..10)
    }

    while stop_rx.try_recv().is_err() {
        tokio::time::sleep(Duration::from_secs(random_seconds())).await;
        worker();
    }
}

#[tokio::test]
#[tracing::instrument]
async fn service_is_responsive_to_shard_changes() {
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let chaos = tokio::task::spawn_blocking(|| async {
        make_env_unstable(stop_rx).await;
    });

    let template_id = DEPS.store_template("option-service").await;

    let mut worker_ids = Vec::new();

    for n in 1..=4 {
        println!("*** WORKER {n} STARTING ***");
        let worker_name = format!("sharding-test-1-{n}");
        let worker_id = DEPS.start_worker(&template_id, &worker_name).await;
        println!("*** WORKER {n} STARTED ***");
        worker_ids.push(worker_id);
    }

    println!("*** ALL WORKERS STARTED ***");

    async fn invoke_and_await_workers(
        workers: &[WorkerId],
    ) -> Result<(), worker::worker_error::Error> {
        let mut tasks = Vec::new();

        for worker_id in workers {
            let worker_id_clone = worker_id.clone();
            tasks.push(tokio::spawn(async move {
                let invocation_key = DEPS.get_invocation_key(&worker_id_clone).await;
                DEPS.invoke_and_await_with_key(
                    &worker_id_clone,
                    &invocation_key,
                    "golem:it/api/echo",
                    vec![Value::Option(Some(Box::new(Value::String(
                        "Hello".to_string(),
                    ))))],
                )
                .await
            }));
        }

        for task in tasks {
            let _ = task.await.unwrap()?;
        }

        Ok(())
    }

    for c in 0..2 {
        if c != 0 {
            std::thread::sleep(Duration::from_secs(10));
        }
        println!("*** INVOKING WORKERS {c} ***");
        invoke_and_await_workers(&worker_ids)
            .await
            .expect("Invocations failed");
        println!("*** INVOKING WORKERS {c} DONE ***");
    }

    println!("*** TEST COMPLETED ***");
    stop_tx.send(()).unwrap();
    chaos.abort();
}
