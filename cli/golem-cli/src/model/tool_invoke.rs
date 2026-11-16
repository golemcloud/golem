// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1

use crate::model::cli_output::StructuredOutput;
use crate::model::text_format::{TextOutput, logln};
use golem_client::model::NativeToolInvocationResponse;
use golem_common::model::InvocationId;
use golem_common::model::invocation_session_public::{
    PublicInvocationResult, PublicNativeToolTarget,
};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolInvokeView {
    pub response: NativeToolInvocationResponse,
}

impl StructuredOutput for ToolInvokeView {
    const KIND: &'static str = "tool.invoke";
}

impl TextOutput for ToolInvokeView {
    fn log(&self) {
        logln(serde_json::to_string_pretty(&self.response).expect("serializable tool response"));
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolInvocationSessionView {
    pub target: PublicNativeToolTarget,
    pub idempotency_key: InvocationId,
    pub result: PublicInvocationResult,
}

impl StructuredOutput for ToolInvocationSessionView {
    const KIND: &'static str = "tool.invoke-session";
}

impl TextOutput for ToolInvocationSessionView {
    fn log(&self) {
        logln(serde_json::to_string_pretty(self).expect("serializable tool session result"));
    }
}

/// The client terminal of an interactive start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientTerminal {
    pub term: Option<String>,
    pub columns: u16,
    pub lines: u16,
}

impl ClientTerminal {
    /// The terminal that stdin and stdout are both attached to, or none.
    pub fn current() -> Option<Self> {
        use std::io::IsTerminal;

        if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
            return None;
        }
        let (columns, lines) = crossterm::terminal::size().ok()?;
        Some(Self {
            term: std::env::var("TERM").ok().filter(|term| !term.is_empty()),
            columns,
            lines,
        })
    }
}

/// The env overlay that an interactive start sends, once and never on attach: exactly the
/// client terminal's `TERM`, `COLUMNS` and `LINES`, and nothing without a terminal. Golem
/// passes them on as ordinary environment variables, so the overlay carries no other variable
/// of the client.
pub fn terminal_env_overlay(terminal: Option<&ClientTerminal>) -> BTreeMap<String, String> {
    let Some(terminal) = terminal else {
        return BTreeMap::new();
    };
    let mut overlay = BTreeMap::from([
        ("COLUMNS".to_string(), terminal.columns.to_string()),
        ("LINES".to_string(), terminal.lines.to_string()),
    ]);
    if let Some(term) = &terminal.term {
        overlay.insert("TERM".to_string(), term.clone());
    }
    overlay
}

#[cfg(test)]
mod tests {
    use super::{ClientTerminal, terminal_env_overlay};
    use std::collections::BTreeMap;
    use test_r::test;

    #[test]
    fn terminal_env_overlay_is_exactly_the_terminal_values() {
        let terminal = ClientTerminal {
            term: Some("xterm-256color".to_string()),
            columns: 132,
            lines: 41,
        };
        assert_eq!(
            terminal_env_overlay(Some(&terminal)),
            BTreeMap::from([
                ("COLUMNS".to_string(), "132".to_string()),
                ("LINES".to_string(), "41".to_string()),
                ("TERM".to_string(), "xterm-256color".to_string()),
            ])
        );
        let without_term = ClientTerminal {
            term: None,
            ..terminal
        };
        assert_eq!(
            terminal_env_overlay(Some(&without_term))
                .into_keys()
                .collect::<Vec<_>>(),
            ["COLUMNS", "LINES"]
        );
        assert!(terminal_env_overlay(None).is_empty());
    }
}
