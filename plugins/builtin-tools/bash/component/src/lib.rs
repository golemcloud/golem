//! Finite shell execution in an existing agent's filesystem and tool context.
//!
//! Every call owns a fresh shell, like a new `bash -c`. The caller chooses only the working
//! directory it starts in; variables, functions, aliases, options, traps, jobs and process
//! numbers end with the call. Permissions, active tasks and human workflows never cross calls.

use bash_shell::session::{Session, check_working_dir};
use golem_rust::{FromSchema, IntoSchema, ToolError, tool_definition, tool_implementation};
use std::fmt::Write as _;

#[cfg(any(target_arch = "wasm32", test))]
mod adapter;
#[cfg(target_arch = "wasm32")]
mod execution;

/// Smallest `$$` given to a new session; lower numbers look like system processes.
const MIN_SHELL_PID: i32 = 1_000;
/// Linux's default `pid_max` ceiling.
const MAX_PID: i32 = 4_194_303;
/// How many seconds a call runs before it is stopped, unless the caller says otherwise.
pub const DEFAULT_TIMEOUT: u32 = 600;
/// The longest time limit a caller may ask for, in seconds.
pub const MAX_TIMEOUT: u32 = 3_600;

/// Maps a random draw uniformly onto the documented `$$` range.
pub fn new_shell_pid(random: u64) -> i32 {
    let span = u64::try_from(MAX_PID - MIN_SHELL_PID + 1).unwrap_or(1);
    MIN_SHELL_PID + i32::try_from(random % span).unwrap_or(0)
}

#[cfg(target_arch = "wasm32")]
fn random_u64() -> u64 {
    // Golem records guest randomness, so replay draws the same number.
    golem_rust::wasip3::random::random::get_random_u64()
}

#[cfg(not(target_arch = "wasm32"))]
fn random_u64() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish()
}

#[derive(Debug, Clone, IntoSchema, FromSchema)]
pub struct BashResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: u8,
    /// Where the script ended; pass it as `cwd` to continue there.
    pub cwd: String,
}

#[derive(Debug, Clone, ToolError)]
pub enum BashError {
    #[tool_error(kind = "usage-error", exit_code = 2)]
    InvalidCwd { reason: String },
    #[tool_error(kind = "usage-error", exit_code = 2)]
    InvalidTimeout { reason: String },
    #[tool_error(kind = "runtime-error", exit_code = 1)]
    Internal { reason: String },
}

#[tool_definition(version = "0.2.0")]
pub trait Bash {
    /// Execute a script in a fresh shell; pass a previous result's cwd to start in that directory.
    /// A script still running after `timeout` seconds (at most 3600) is stopped with TERM, then
    /// KILL a second later, and the call returns exit code 124.
    #[arg(cwd = "option", default = "")]
    #[arg(script = "positional")]
    #[arg(timeout = "option", default = 600)]
    async fn run(&self, cwd: String, script: String, timeout: u32)
    -> Result<BashResult, BashError>;
}

struct BashImpl;

#[tool_implementation]
impl Bash for BashImpl {
    async fn run(
        &self,
        cwd: String,
        script: String,
        timeout: u32,
    ) -> Result<BashResult, BashError> {
        run_script(&cwd, &script, timeout).await
    }
}

pub async fn run_script(cwd: &str, script: &str, timeout: u32) -> Result<BashResult, BashError> {
    #[cfg(target_arch = "wasm32")]
    {
        // golem-rust installs its logger only in an agent's constructor; a tool component has
        // none. A second install fails harmlessly: the first logger stands.
        let _ = wasi_logger::Logger::install();
        log::set_max_level(log::LevelFilter::Info);
    }
    // Check the directory before anything runs; a bad one must never fall back to executing
    // the script somewhere else.
    if !cwd.is_empty() {
        check_working_dir(cwd).map_err(rejected_cwd)?;
    }
    if !(1..=MAX_TIMEOUT).contains(&timeout) {
        let reason = format!("timeout {timeout}: must be between 1 and {MAX_TIMEOUT} seconds");
        log::warn!("timeout rejected: {reason}");
        return Err(BashError::InvalidTimeout { reason });
    }
    #[cfg(target_arch = "wasm32")]
    let session = Session::new_with_execution_services(execution::services()).await;
    #[cfg(not(target_arch = "wasm32"))]
    let session = Session::new().await;
    let mut session = session.map_err(|error| {
        log::error!("cannot start shell: {error}");
        BashError::Internal {
            reason: format!("cannot start shell: {error}"),
        }
    })?;
    #[cfg(target_arch = "wasm32")]
    let shadowed =
        adapter::install(&mut session).map_err(|reason| BashError::Internal { reason })?;
    #[cfg(not(target_arch = "wasm32"))]
    let shadowed: Vec<String> = vec![];
    // Each call is a new process with its own `$$`, so two calls writing `/tmp/out.$$` at the
    // same time do not collide.
    let shell_pid = new_shell_pid(random_u64());
    session.seed_process_ids(shell_pid, shell_pid + 1);
    if !cwd.is_empty() {
        session.set_working_dir(cwd).map_err(rejected_cwd)?;
    }
    session.set_time_limit(std::time::Duration::from_secs(u64::from(timeout)));

    let result = session.run(script).await;
    let cwd = session.cwd().display().to_string();
    let mut stderr = String::new();
    for name in shadowed {
        log::warn!("bound tool {name:?} is shadowed by a local command");
        let _ = writeln!(
            stderr,
            "bash: bound tool {name:?} is shadowed by a local command"
        );
    }
    stderr.push_str(&String::from_utf8_lossy(&result.stderr));
    Ok(BashResult {
        stdout: String::from_utf8_lossy(&result.stdout).into_owned(),
        stderr,
        exit_code: result.exit_code,
        cwd,
    })
}

fn rejected_cwd(reason: String) -> BashError {
    log::warn!("cwd rejected: {reason}");
    BashError::InvalidCwd { reason }
}

#[cfg(test)]
mod tests;
