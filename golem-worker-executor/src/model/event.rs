use golem_common::model::oplog::OplogEntry;
use golem_common::model::{AgentEvent, LogLevel, OplogIndex, Timestamp, oplog};
use uuid::Uuid;

// Internal version of AgentEvent, without any operational details.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum InternalWorkerEvent {
    StdOut {
        timestamp: Timestamp,
        bytes: Vec<u8>,
        invocation_id: Option<Uuid>,
    },
    StdErr {
        timestamp: Timestamp,
        bytes: Vec<u8>,
        invocation_id: Option<Uuid>,
    },
    Log {
        timestamp: Timestamp,
        level: LogLevel,
        context: String,
        message: String,
        invocation_id: Option<Uuid>,
    },
    InvocationStart {
        timestamp: Timestamp,
        function: String,
        invocation_id: Uuid,
    },
    InvocationFinished {
        timestamp: Timestamp,
        function: String,
        invocation_id: Uuid,
    },
    PluginError {
        timestamp: Timestamp,
        plugin_name: String,
        message: String,
    },
    SnapshotRecoverySucceeded {
        timestamp: Timestamp,
        snapshot_index: OplogIndex,
    },
    SnapshotRecoveryFailed {
        timestamp: Timestamp,
        snapshot_index: OplogIndex,
        error: String,
    },
}

impl InternalWorkerEvent {
    pub fn stdout(bytes: Vec<u8>, invocation_id: Option<Uuid>) -> Self {
        Self::StdOut {
            timestamp: Timestamp::now_utc(),
            bytes,
            invocation_id,
        }
    }

    pub fn stderr(bytes: Vec<u8>, invocation_id: Option<Uuid>) -> Self {
        Self::StdErr {
            timestamp: Timestamp::now_utc(),
            bytes,
            invocation_id,
        }
    }

    pub fn log(level: LogLevel, context: &str, message: &str, invocation_id: Option<Uuid>) -> Self {
        Self::Log {
            timestamp: Timestamp::now_utc(),
            level,
            context: context.to_string(),
            message: message.to_string(),
            invocation_id,
        }
    }

    pub fn invocation_start(function: &str, invocation_id: Uuid) -> Self {
        Self::InvocationStart {
            timestamp: Timestamp::now_utc(),
            function: function.to_string(),
            invocation_id,
        }
    }

    pub fn invocation_finished(function: &str, invocation_id: Uuid) -> Self {
        Self::InvocationFinished {
            timestamp: Timestamp::now_utc(),
            function: function.to_string(),
            invocation_id,
        }
    }

    pub fn plugin_error(plugin_name: &str, message: &str) -> Self {
        Self::PluginError {
            timestamp: Timestamp::now_utc(),
            plugin_name: plugin_name.to_string(),
            message: message.to_string(),
        }
    }

    pub fn snapshot_recovery_succeeded(snapshot_index: OplogIndex) -> Self {
        Self::SnapshotRecoverySucceeded {
            timestamp: Timestamp::now_utc(),
            snapshot_index,
        }
    }

    pub fn snapshot_recovery_failed(snapshot_index: OplogIndex, error: String) -> Self {
        Self::SnapshotRecoveryFailed {
            timestamp: Timestamp::now_utc(),
            snapshot_index,
            error,
        }
    }

    pub fn as_oplog_entry(&self) -> Option<OplogEntry> {
        match self {
            Self::StdOut {
                timestamp, bytes, ..
            } => Some(OplogEntry::Log {
                timestamp: *timestamp,
                parent_start_index: None,
                level: oplog::LogLevel::Stdout,
                context: String::new(),
                message: String::from_utf8_lossy(bytes).to_string(),
                trace_context: None,
            }),
            Self::StdErr {
                timestamp, bytes, ..
            } => Some(OplogEntry::Log {
                timestamp: *timestamp,
                parent_start_index: None,
                level: oplog::LogLevel::Stderr,
                context: String::new(),
                message: String::from_utf8_lossy(bytes).to_string(),
                trace_context: None,
            }),
            Self::Log {
                timestamp,
                level,
                context,
                message,
                ..
            } => Some(OplogEntry::Log {
                timestamp: *timestamp,
                parent_start_index: None,
                level: match level {
                    LogLevel::Trace => oplog::LogLevel::Trace,
                    LogLevel::Debug => oplog::LogLevel::Debug,
                    LogLevel::Info => oplog::LogLevel::Info,
                    LogLevel::Warn => oplog::LogLevel::Warn,
                    LogLevel::Error => oplog::LogLevel::Error,
                    LogLevel::Critical => oplog::LogLevel::Critical,
                },
                context: context.clone(),
                message: message.clone(),
                trace_context: None,
            }),
            Self::InvocationStart { .. } => None,
            Self::InvocationFinished { .. } => None,
            Self::PluginError { .. } => None,
            Self::SnapshotRecoverySucceeded { .. } => None,
            Self::SnapshotRecoveryFailed { .. } => None,
        }
    }
}

impl From<InternalWorkerEvent> for AgentEvent {
    fn from(event: InternalWorkerEvent) -> Self {
        match event {
            InternalWorkerEvent::StdOut {
                timestamp,
                bytes,
                invocation_id,
            } => Self::StdOut {
                timestamp,
                bytes,
                invocation_id,
            },
            InternalWorkerEvent::StdErr {
                timestamp,
                bytes,
                invocation_id,
            } => Self::StdErr {
                timestamp,
                bytes,
                invocation_id,
            },
            InternalWorkerEvent::Log {
                timestamp,
                level,
                context,
                message,
                invocation_id,
            } => Self::Log {
                timestamp,
                level,
                context,
                message,
                invocation_id,
            },
            InternalWorkerEvent::InvocationStart {
                timestamp,
                function,
                invocation_id,
            } => Self::InvocationStart {
                timestamp,
                function,
                invocation_id,
            },
            InternalWorkerEvent::InvocationFinished {
                timestamp,
                function,
                invocation_id,
            } => Self::InvocationFinished {
                timestamp,
                function,
                invocation_id,
            },
            InternalWorkerEvent::PluginError {
                timestamp,
                plugin_name,
                message,
            } => Self::PluginError {
                timestamp,
                plugin_name,
                message,
            },
            InternalWorkerEvent::SnapshotRecoverySucceeded {
                timestamp,
                snapshot_index,
            } => Self::SnapshotRecoverySucceeded {
                timestamp,
                snapshot_index,
            },
            InternalWorkerEvent::SnapshotRecoveryFailed {
                timestamp,
                snapshot_index,
                error,
            } => Self::SnapshotRecoveryFailed {
                timestamp,
                snapshot_index,
                error,
            },
        }
    }
}
