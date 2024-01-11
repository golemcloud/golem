use std::net::SocketAddr;

use http::{Response, StatusCode};
use tokio::task::JoinHandle;
use warp::hyper::Body;
use warp::Filter;

pub struct HttpServerImpl {
    #[allow(dead_code)]
    handle: JoinHandle<()>,
}

impl HttpServerImpl {
    pub fn new(addr: impl Into<SocketAddr> + Send + 'static) -> HttpServerImpl {
        let handle = tokio::spawn(server(addr));
        HttpServerImpl { handle }
    }
}

async fn server(addr: impl Into<SocketAddr> + Send) {
    let healthcheck = warp::path!("healthcheck").map(|| {
        Response::builder()
            .status(StatusCode::OK)
            .body(Body::from("shard manager is running"))
            .unwrap()
    });

    warp::serve(healthcheck).run(addr).await;
}
