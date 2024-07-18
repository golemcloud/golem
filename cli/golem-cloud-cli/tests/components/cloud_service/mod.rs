use crate::components::rdb::CloudDbInfo;
use crate::components::ROOT_TOKEN;
use async_trait::async_trait;
use golem_test_framework::components::rdb::Rdb;
use golem_test_framework::components::redis::Redis;
use golem_test_framework::components::wait_for_startup_grpc;
use indoc::formatdoc;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tracing::Level;

pub mod spawned;

#[async_trait]
pub trait CloudService {
    fn private_host(&self) -> String;
    fn private_http_port(&self) -> u16;
    fn private_grpc_port(&self) -> u16;
    fn public_host(&self) -> String {
        self.private_host()
    }
    fn public_http_port(&self) -> u16 {
        self.private_http_port()
    }
    fn public_grpc_port(&self) -> u16 {
        self.private_grpc_port()
    }
    fn kill(&self);
}

async fn wait_for_startup(host: &str, grpc_port: u16, timeout: Duration) {
    wait_for_startup_grpc(host, grpc_port, "cloud-service", timeout).await
}

async fn env_vars(
    http_port: u16,
    grpc_port: u16,
    redis: Arc<dyn Redis + Send + Sync + 'static>,
    rdb: Arc<dyn Rdb + Send + Sync + 'static>,
    verbosity: Level,
) -> HashMap<String, String> {
    let log_level = verbosity.as_str().to_lowercase();

    let priv_key = formatdoc!(
        "
        -----BEGIN PRIVATE KEY-----
        MC4CAQAwBQYDK2VwBCIEIASoJkiCzW7yLb8gZnQ/d1bCL19IM+7qnGl5r8Fwm2NI
        -----END PRIVATE KEY-----
        "
    );

    let pub_key = formatdoc!(
        "
        -----BEGIN PUBLIC KEY-----
        MCowBQYDK2VwAyEAiZicK0lnJhb8Qi6ByjTKXziYsA7fL3kWqEaNNAVIzZg=
        -----END PUBLIC KEY-----
        "
    );

    let env: &[(&str, &str)] = &[
        ("ENVIRONMENT", "local"),
        ("GOLEM__ENVIRONMENT", "local"),
        ("GOLEM__WORKSPACE", "it"),
        ("RUST_LOG", &format!("{log_level},h2=warn,cranelift_codegen=warn,wasmtime_cranelift=warn,wasmtime_jit=warn")),
        ("WASMTIME_BACKTRACE_DETAILS", "1"),
        ("RUST_BACKTRACE", "1"),
        ("GOLEM__GRPC_PORT", &grpc_port.to_string()),
        ("GOLEM__HTTP_PORT", &http_port.to_string()),
        ("GOLEM__TRACING__STDOUT__JSON", "true"),
        ("GOLEM__REDIS__HOST", &redis.private_host()),
        ("GOLEM__REDIS__PORT", &redis.private_port().to_string()),
        ("GOLEM__REDIS__KEY_PREFIX", redis.prefix()),
        ("GOLEM__ED_DSA__PRIVATE_KEY", &priv_key),
        ("GOLEM__ED_DSA__PUBLIC_KEY", &pub_key),
        ("GOLEM__ACCOUNTS__ROOT__TOKEN", ROOT_TOKEN),
        ("GOLEM__ACCOUNTS__MARKETING__TOKEN", "4a029650-26a7-4220-bfee-aa386fba08da"),
    ];

    let mut vars = HashMap::from_iter(env.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    vars.extend(rdb.info().cloud_env("cloud_service").await.clone());
    vars
}
