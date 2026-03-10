// Copyright 2024-2026 Golem Cloud
//
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://license.golem.cloud/LICENSE
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::components::redis::Redis;
use crate::components::ChildProcessLogger;
use async_trait::async_trait;
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tempfile::TempDir;
use tracing::{info, Level};

/// Pre-generated CA certificate (valid 10 years) that signed the server cert.
/// Use this to build a `RootCertStore` when connecting clients in tests.
pub const CERT_PEM: &[u8] = include_bytes!("tls/ca.crt");

/// Server certificate signed by the CA above.
const SERVER_CERT_PEM: &[u8] = include_bytes!("tls/redis.crt");
const KEY_PEM: &[u8] = include_bytes!("tls/redis.key");

pub struct SpawnedRedisTls {
    port: u16,
    prefix: String,
    child: Arc<Mutex<Option<Child>>>,
    valid: AtomicBool,
    _logger: ChildProcessLogger,
    /// Keeps the temp directory (and its cert/key files) alive for the lifetime of the server.
    _cert_dir: TempDir,
}

impl SpawnedRedisTls {
    pub fn new(port: u16, prefix: String, out_level: Level, err_level: Level) -> Self {
        info!("Starting TLS Redis on port {}", port);

        let cert_dir = TempDir::new().expect("Failed to create temp dir for TLS certs");

        let ca_path = cert_dir.path().join("ca.crt");
        let cert_path = cert_dir.path().join("redis.crt");
        let key_path = cert_dir.path().join("redis.key");

        std::fs::write(&ca_path, CERT_PEM).expect("Failed to write ca.crt to temp dir");
        std::fs::write(&cert_path, SERVER_CERT_PEM).expect("Failed to write redis.crt to temp dir");
        std::fs::write(&key_path, KEY_PEM).expect("Failed to write redis.key to temp dir");

        let mut child = Command::new("redis-server")
            // Disable plain-text port
            .arg("--port")
            .arg("0")
            // TLS port
            .arg("--tls-port")
            .arg(port.to_string())
            .arg("--tls-cert-file")
            .arg(&cert_path)
            .arg("--tls-key-file")
            .arg(&key_path)
            .arg("--tls-ca-cert-file")
            .arg(&ca_path)
            // Don't require clients to present a cert
            .arg("--tls-auth-clients")
            .arg("no")
            // Disable persistence
            .arg("--save")
            .arg("")
            .arg("--appendonly")
            .arg("no")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("Failed to spawn TLS redis server");

        let logger =
            ChildProcessLogger::log_child_process("[redis-tls]", out_level, err_level, &mut child);

        wait_for_tls_startup("localhost", port, Duration::from_secs(10));

        Self {
            port,
            prefix,
            child: Arc::new(Mutex::new(Some(child))),
            valid: AtomicBool::new(true),
            _logger: logger,
            _cert_dir: cert_dir,
        }
    }

    fn blocking_kill(&self) {
        info!("Stopping TLS Redis");
        if let Some(mut child) = self.child.lock().unwrap().take() {
            self.valid.store(false, Ordering::Release);
            let _ = child.kill();
        }
    }
}

/// Wait until the TCP port accepts a connection. We cannot use the `redis` crate here because
/// the port speaks TLS rather than plain Redis protocol.
fn wait_for_tls_startup(host: &str, port: u16, timeout: Duration) {
    info!(
        "Waiting for TLS Redis start on host {host}:{port}, timeout: {}s",
        timeout.as_secs()
    );
    let addr = format!("{host}:{port}");
    let start = Instant::now();
    loop {
        if TcpStream::connect(&addr).is_ok() {
            break;
        }
        if start.elapsed() > timeout {
            panic!("Failed to verify that TLS Redis is running on {addr}");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[async_trait]
impl Redis for SpawnedRedisTls {
    fn assert_valid(&self) {
        if !self.valid.load(Ordering::Acquire) {
            std::panic!("TLS Redis has been closed")
        }
    }

    fn private_host(&self) -> String {
        "localhost".to_string()
    }

    fn private_port(&self) -> u16 {
        self.port
    }

    fn prefix(&self) -> &str {
        &self.prefix
    }

    async fn kill(&self) {
        self.blocking_kill();
    }
}

impl Drop for SpawnedRedisTls {
    fn drop(&mut self) {
        self.blocking_kill();
    }
}
