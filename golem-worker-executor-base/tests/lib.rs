use ctor::{ctor, dtor};

use redis::{Commands, RedisResult};
use std::ops::Deref;
use std::panic;
use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::prelude::*;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

#[allow(dead_code)]
mod common;

pub mod api;
pub mod blobstore;
pub mod guest_languages;
pub mod keyvalue;
pub mod rpc;
pub mod scalability;
pub mod transactions;
pub mod wasi;

#[allow(dead_code)]
struct Redis {
    pub host: String,
    pub port: u16,
    child: Option<Child>,
    valid: AtomicBool,
}

impl Redis {
    pub fn new() -> Self {
        let port = 6379;
        let child = Command::new("redis-server")
            .arg("--port")
            .arg(port.to_string())
            .arg("--save")
            .arg("")
            .arg("--appendonly")
            .arg("no")
            .spawn()
            .expect("Failed to spawn redis server");

        let start = Instant::now();
        let mut client = redis::Client::open("redis://localhost:6379").unwrap();
        loop {
            let result: RedisResult<Vec<String>> = client.keys("*");
            if result.is_ok() {
                break;
            }

            if start.elapsed().as_secs() > 10 {
                panic!("Failed to verify that Redis is running");
            }
        }

        Self {
            host: "localhost".to_string(),
            port,
            child: Some(child),
            valid: AtomicBool::new(true),
        }
    }

    pub fn assert_valid(&self) {
        if !self.valid.load(Ordering::Acquire) {
            panic!("Redis has been closed")
        }
    }

    pub fn kill(&mut self) {
        if let Some(mut child) = self.child.take() {
            self.valid.store(false, Ordering::Release);
            let _ = child.kill();
        }
    }
}

impl Drop for Redis {
    fn drop(&mut self) {
        self.kill();
    }
}

#[ctor]
pub static REDIS: Redis = Redis::new();

#[dtor]
unsafe fn drop_redis() {
    let redis_ptr = REDIS.deref() as *const Redis;
    let redis_ptr = redis_ptr as *mut Redis;
    (*redis_ptr).kill()
}

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
pub static TRACING: Tracing = Tracing::init();
