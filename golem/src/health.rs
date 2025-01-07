// Copyright 2024-2025 Golem Cloud
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use hyper::StatusCode;
use poem::endpoint::PrometheusExporter;
use poem::listener::{Acceptor, Listener};
use poem::{get, Endpoint, Error, Route};
use reqwest::Client;
use tokio::task::JoinSet;
use tracing::info;

pub async fn start_healthcheck_server(
    ports: Vec<u16>,
    prometheus_registry: prometheus::Registry,
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
) -> Result<u16, anyhow::Error> {
    let healtcheck_endpoint = HealthcheckApi::new(ports);
    let metrics = PrometheusExporter::new(prometheus_registry.clone());

    let app = Route::new()
        .at("/healthcheck", get(healtcheck_endpoint))
        .at("/metrics", metrics);

    let poem_listener = poem::listener::TcpListener::bind("0.0.0.0:0");
    let acceptor = poem_listener.into_acceptor().await?;
    let port = acceptor.local_addr()[0]
        .as_socket_addr()
        .expect("socket address")
        .port();
    let server = poem::Server::new_with_acceptor(acceptor);

    join_set.spawn(async move { server.run(app).await.map_err(|e| e.into()) });

    info!("Healthcheck server started on {port}");
    Ok(port)
}

struct HealthcheckApi {
    ports: Vec<u16>,
    client: Client,
}

impl HealthcheckApi {
    fn new(ports: Vec<u16>) -> Self {
        Self {
            ports,
            client: Client::new(),
        }
    }

    async fn check_one(&self, port: u16) -> poem::Result<()> {
        let response = self
            .client
            .get(format!("http://127.0.0.1:{port}/healthcheck"))
            .send()
            .await;

        match response {
            Ok(response) if response.status().is_success() => Ok(()),
            _ => Err(Error::from_string(
                format!("health check failed for {port}"),
                StatusCode::INTERNAL_SERVER_ERROR,
            )),
        }
    }
}

impl Endpoint for HealthcheckApi {
    type Output = ();

    async fn call(&self, _req: poem::Request) -> poem::Result<Self::Output> {
        for port in self.ports.iter() {
            self.check_one(*port).await?;
        }
        Ok(())
    }
}
