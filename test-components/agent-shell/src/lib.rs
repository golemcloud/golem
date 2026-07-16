//! A minimal implementer of the `golem agent shell` surface, used to integration-test the CLI's
//! wire contract against a real SDK-built component.
//!
//! The surface: `eval(string)`, `answer_prompt(string)`, `abort_prompt()` — each returning the
//! `eval-result` record `{ stdout, stderr, exit-code, pending-prompt }`, where `pending-prompt`
//! carries an outstanding question the caller must resolve on a *separate* invocation (agent
//! invocations are serialized per instance, so an agent may never block waiting on a human).
//!
//! `eval` implements a tiny deterministic command language chosen to exercise every branch the CLI
//! has: plain output, failure exit codes, durable per-instance state, both question flavors, the
//! eval-while-pending rejection (with the prompt re-surfaced so a fresh client can recover), and
//! answer rejection for out-of-choice answers.

use golem_rust::{FromSchema, IntoSchema, agent_definition, agent_implementation};

/// The result of one shell-surface call. Field order is the wire contract: golem's value encoding
/// is positional (names live in the type graph, not the value), and the CLI decodes this record by
/// position — `stdout`, `stderr`, `exit_code`, `pending_prompt`.
#[derive(Clone, IntoSchema, FromSchema)]
pub struct EvalResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: u8,
    pub pending_prompt: Option<PendingPrompt>,
}

/// An outstanding question. Field order is the wire contract — `question`, `choices`.
#[derive(Clone, IntoSchema, FromSchema)]
pub struct PendingPrompt {
    pub question: String,
    pub choices: Option<Vec<String>>,
}

#[agent_definition]
trait ShellSurface {
    fn new(id: String) -> Self;

    /// Run one command line. Commands: `echo <text>`, `count` (durable per-instance counter),
    /// `fail` (exit 1), `ask-choice` (surface a question with choices), `ask-free` (surface a
    /// free-form question). While a question is pending every `eval` is rejected (exit 1) with the
    /// prompt re-surfaced.
    async fn eval(&mut self, cmd: String) -> EvalResult;

    /// Deliver an answer to the outstanding question. An answer outside the allowed choices is
    /// rejected (exit 1) and the question stays pending.
    async fn answer_prompt(&mut self, response: String) -> EvalResult;

    /// Cancel the outstanding question (exit 130, the Ctrl-C convention).
    async fn abort_prompt(&mut self) -> EvalResult;
}

struct ShellSurfaceImpl {
    _id: String,
    /// Durable per-instance state, proving a shell session accumulates across invocations.
    count: u32,
    pending: Option<PendingPrompt>,
}

fn ok(stdout: String) -> EvalResult {
    EvalResult {
        stdout,
        stderr: String::new(),
        exit_code: 0,
        pending_prompt: None,
    }
}

fn err(stderr: &str, exit_code: u8, pending_prompt: Option<PendingPrompt>) -> EvalResult {
    EvalResult {
        stdout: String::new(),
        stderr: stderr.to_string(),
        exit_code,
        pending_prompt,
    }
}

#[agent_implementation]
impl ShellSurface for ShellSurfaceImpl {
    fn new(id: String) -> Self {
        Self {
            _id: id,
            count: 0,
            pending: None,
        }
    }

    async fn eval(&mut self, cmd: String) -> EvalResult {
        // While a question is pending, evals are rejected and the prompt RE-SURFACES, so a client
        // that attaches mid-question (e.g. after a previous session left it dangling) can recover.
        if let Some(pending) = &self.pending {
            return err("a question is pending\n", 1, Some(pending.clone()));
        }

        match cmd.trim() {
            "count" => {
                self.count += 1;
                ok(format!("{}\n", self.count))
            }
            "fail" => err("failed\n", 1, None),
            "ask-choice" => {
                let prompt = PendingPrompt {
                    question: "Proceed?".to_string(),
                    choices: Some(vec!["yes".to_string(), "no".to_string()]),
                };
                self.pending = Some(prompt.clone());
                EvalResult {
                    stdout: "Proceed?\n".to_string(),
                    stderr: String::new(),
                    exit_code: 0,
                    pending_prompt: Some(prompt),
                }
            }
            "ask-free" => {
                let prompt = PendingPrompt {
                    question: "Your name?".to_string(),
                    choices: None,
                };
                self.pending = Some(prompt.clone());
                EvalResult {
                    stdout: "Your name?\n".to_string(),
                    stderr: String::new(),
                    exit_code: 0,
                    pending_prompt: Some(prompt),
                }
            }
            other => match other.strip_prefix("echo ") {
                Some(text) => ok(format!("{text}\n")),
                None => err(&format!("unknown command: {other}\n"), 127, None),
            },
        }
    }

    async fn answer_prompt(&mut self, response: String) -> EvalResult {
        let Some(pending) = &self.pending else {
            return err("no question pending\n", 1, None);
        };

        // An answer outside the allowed choices is rejected and the question STAYS pending — the
        // CLI's resolve loop re-asks until a valid answer or an abort arrives.
        if let Some(choices) = &pending.choices
            && !choices.contains(&response)
        {
            return err("not an allowed choice\n", 1, Some(pending.clone()));
        }

        self.pending = None;
        ok(format!("answered: {response}\n"))
    }

    async fn abort_prompt(&mut self) -> EvalResult {
        if self.pending.take().is_none() {
            return err("no question pending\n", 1, None);
        }
        err("aborted\n", 130, None)
    }
}
