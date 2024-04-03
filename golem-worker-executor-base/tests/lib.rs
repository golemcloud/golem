use std::ops::Deref;

use ctor::{ctor, dtor};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::prelude::*;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

use golem_test_framework::config::{TestDependencies, EnvBasedTestDependencies};

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

#[ctor]
pub static DOCKER: testcontainers::clients::Cli = testcontainers::clients::Cli::default();

#[ctor]
pub static CONFIG: EnvBasedTestDependencies = EnvBasedTestDependencies::new(&DOCKER);

#[dtor]
unsafe fn drop_config() {
    let config_ptr = CONFIG.deref() as *const EnvBasedTestDependencies;
    let config_ptr = config_ptr as *mut EnvBasedTestDependencies;
    (*config_ptr).redis().kill();
    (*config_ptr).redis_monitor().kill();
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
