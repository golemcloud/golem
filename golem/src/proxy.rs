// Copyright 2024 Golem Cloud
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

use std::net::Ipv4Addr;
use std::sync::Arc;
use actix_web::{web, App, HttpServer, HttpResponse, HttpRequest};
use anyhow::Context;
use tokio::task::JoinSet;
use tracing::info;

use crate::AllRunDetails;

#[derive(Debug, Clone)]
pub enum PathRule {
    Equals(String),
    Prefix(String),
    Regex(String),
}

impl PathRule {
    pub fn equals(path: &str) -> Self {
        PathRule::Equals(path.to_string())
    }

    pub fn prefix(path: &str) -> Self {
        PathRule::Prefix(path.to_string())
    }

    pub fn regex(path: &str) -> Self {
        PathRule::Regex(path.to_string())
    }
}

#[derive(Clone)]
struct AppState {
    routes: Arc<Vec<(PathRule, String)>>,
    backend_ports: Arc<std::collections::HashMap<String, u16>>,
}

pub async fn start_proxy(
    listener_addr: &str,
    listener_port: u16,
    healthcheck_port: u16,
    all_run_details: &AllRunDetails,
    join_set: &mut JoinSet<Result<(), anyhow::Error>>,
) -> Result<(), anyhow::Error> {
    info!("Starting Actix-web proxy");

    let ipv4_addr: Ipv4Addr = listener_addr.parse().context(format!(
        "Failed at parsing the listener host address {}",
        listener_addr
    ))?;

    let component_backend = "golem-component";
    let worker_backend = "golem-worker";
    let health_backend = "golem-health";

    // Initialize backend ports
    let mut backend_ports = std::collections::HashMap::new();
    backend_ports.insert(health_backend.to_string(), healthcheck_port);
    backend_ports.insert(
        component_backend.to_string(),
        all_run_details.component_service.http_port,
    );
    backend_ports.insert(
        worker_backend.to_string(),
        all_run_details.worker_service.http_port,
    );

    // Initialize routes
    let mut routes = Vec::new();
    routes.push((PathRule::equals("/healthcheck"), health_backend.to_string()));
    routes.push((PathRule::equals("/metrics"), health_backend.to_string()));
    routes.push((
        PathRule::regex("/v1/components/[^/]+/workers/[^/]+/connect$"),
        worker_backend.to_string(),
    ));
    routes.push((PathRule::prefix("/v1/api"), worker_backend.to_string()));
    routes.push((
        PathRule::regex("/v1/components/[^/]+/workers"),
        worker_backend.to_string(),
    ));
    routes.push((
        PathRule::regex("/v1/components/[^/]+/invoke"),
        worker_backend.to_string(),
    ));
    routes.push((
        PathRule::regex("/v1/components/[^/]+/invoke-and-await"),
        worker_backend.to_string(),
    ));
    routes.push((
        PathRule::prefix("/v1/components"),
        component_backend.to_string(),
    ));
    routes.push((PathRule::prefix("/"), component_backend.to_string()));

    let state = AppState {
        routes: Arc::new(routes),
        backend_ports: Arc::new(backend_ports),
    };

    let server = HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(state.clone()))
            .default_service(web::route().to(proxy_handler))
    })
    .bind((ipv4_addr.to_string(), listener_port))?
    .run();

    join_set.spawn(async move {
        server.await.map_err(|e| anyhow::anyhow!("Server error: {}", e))
    });

    Ok(())
}

async fn proxy_handler(
    req: HttpRequest,
    body: web::Bytes,
    state: web::Data<AppState>,
) -> HttpResponse {
    let path = req.uri().path();
    let backend = match find_matching_backend(path, &state.routes) {
        Some(backend) => backend,
        None => return HttpResponse::NotFound().finish(),
    };

    let port = match state.backend_ports.get(backend) {
        Some(port) => port,
        None => return HttpResponse::InternalServerError().finish(),
    };

    let client = reqwest::Client::new();
    let mut proxy_req = client
        .request(
            reqwest::Method::from_bytes(req.method().as_str().as_bytes()).unwrap(),
            format!("http://127.0.0.1:{}{}", port, path),
        )
        .body(body.to_vec());

    // Forward headers
    for (name, value) in req.headers() {
        if name != "host" {
            if let Ok(header_name) = reqwest::header::HeaderName::from_bytes(name.as_str().as_bytes()) {
                if let Ok(header_value) = reqwest::header::HeaderValue::from_str(value.to_str().unwrap_or_default()) {
                    proxy_req = proxy_req.header(header_name, header_value);
                }
            }
        }
    }

    match proxy_req.send().await {
        Ok(resp) => {
            let mut builder = HttpResponse::build(actix_web::http::StatusCode::from_u16(resp.status().as_u16()).unwrap());
            for (name, value) in resp.headers() {
                if !name.as_str().starts_with("connection") {
                    if let Ok(header_value) = actix_web::http::header::HeaderValue::from_bytes(value.as_bytes()) {
                        builder.append_header((name.as_str(), header_value));
                    }
                }
            }
            match resp.bytes().await {
                Ok(bytes) => builder.body(bytes),
                Err(_) => HttpResponse::InternalServerError().finish(),
            }
        }
        Err(_) => HttpResponse::InternalServerError().finish(),
    }
}

fn find_matching_backend<'a>(path: &str, routes: &'a [(PathRule, String)]) -> Option<&'a String> {
    for (rule, backend) in routes {
        let matches = match rule {
            PathRule::Equals(p) => path == p,
            PathRule::Prefix(p) => path.starts_with(p),
            PathRule::Regex(p) => regex::Regex::new(p)
                .map(|re| re.is_match(path))
                .unwrap_or(false),
        };
        if matches {
            return Some(backend);
        }
    }
    None
}

