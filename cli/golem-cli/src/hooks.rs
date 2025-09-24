
use std::sync::Arc;

// Pull the types from the existing module:
use golem_cli::command_handler::{
    CommandHandlerHooks,
    // Only used when the feature is ON:
    #[cfg(feature = "server-commands")]
    Context,
    #[cfg(feature = "server-commands")]
    ServerSubcommand,
    #[cfg(feature = "server-commands")]
    Verbosity,
};

/// Keep it EMPTY to avoid trait/lifetime churn across CommandHandler generics.
pub struct NoHooks;

// When the feature is OFF, the trait has no required methods / empty impl.
#[cfg(not(feature = "server-commands"))]
impl CommandHandlerHooks for NoHooks {}

// When the feature is ON, provide no-op implementations that satisfy signatures.
#[cfg(feature = "server-commands")]
impl CommandHandlerHooks for NoHooks {
    fn handler_server_commands(
        &self,
        _ctx: Arc<Context>,
        _subcommand: ServerSubcommand,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> {
        async { Ok(()) }
    }

    // Used for auto-starting the default server; we do nothing.
    fn run_server() -> impl std::future::Future<Output = anyhow::Result<()>> + Send {
        async { Ok(()) }
    }

    // Don’t change verbosity by default.
    fn override_verbosity(verbosity: Verbosity) -> Verbosity {
        verbosity
    }

    // Don’t force pretty mode.
    fn override_pretty_mode() -> bool {
        false
    }
}
