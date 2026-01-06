use clap::Args;

#[derive(Args, Debug, Clone)]
pub struct ServeArgs {
    /// Port to run the MCP Server on
    #[arg(long, short, default_value = "1232")]
    pub port: u16,
}
