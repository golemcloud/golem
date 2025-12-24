use clap::{Args, Subcommand};

#[derive(Debug, Args, Default)]
pub struct RunArgs {
    /// Port to serve the MCP server on, defaults to 9000
    #[clap(long, short, default_value = "9000")]
    pub port: u16,

    /// Transport to use for the MCP server (e.g., Sse, StreamableHttp)
    #[clap(long, short, default_value = "Sse")]
    pub transport: String,
}

#[derive(Debug, Subcommand)]
pub enum McpServerSubcommand {
    /// Run Golem CLI in MCP server mode
    Run {
        #[clap(flatten)]
        args: RunArgs,
    },
}
