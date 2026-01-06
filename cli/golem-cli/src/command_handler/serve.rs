use std::sync::Arc;
use crate::context::Context;
use crate::command::serve::ServeArgs;
use std::net::SocketAddr;
use tokio::net::TcpListener;

pub struct ServeCommandHandler {
    ctx: Arc<Context>,
}

impl ServeCommandHandler {
    pub fn new(ctx: Arc<Context>) -> Self {
        Self { ctx }
    }

    pub async fn handle(&self, args: ServeArgs) -> anyhow::Result<()> {
        let client = self.ctx.golem_clients().await?;

        // 2. Ignite the Router
        let app = crate::mcp::router::create_router(client);

        // 3. Bind to Port
        let addr = SocketAddr::from(([127, 0, 0, 1], args.port));
        tracing::info!("✓ STATUS: LISTENING on http://{}", addr);

        let listener = TcpListener::bind(addr).await?;
        axum::serve(listener, app).await?;

        Ok(())
    }
}
