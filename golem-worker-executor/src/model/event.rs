use golem_common::model::{oplog, IdempotencyKey, LogLevel, Timestamp, WorkerEvent};
use golem_common::model::oplog::OplogEntry;

// Internal version of WorkerEvent, without any operational details.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum InternalWorkerEvent {
    StdOut {
        timestamp: Timestamp,
        bytes: Vec<u8>,
    },
    StdErr {
        timestamp: Timestamp,
        bytes: Vec<u8>,
    },
    Log {
        timestamp: Timestamp,
        level: LogLevel,
        context: String,
        message: String,
    },
    InvocationStart {
        timestamp: Timestamp,
        function: String,
        idempotency_key: IdempotencyKey,
    },
    InvocationFinished {
        timestamp: Timestamp,
        function: String,
        idempotency_key: IdempotencyKey,
    }
}

impl InternalWorkerEvent {
    pub fn stdout(bytes: Vec<u8>) -> Self {
        Self::StdOut {
            timestamp: Timestamp::now_utc(),
            bytes,
        }
    }

    pub fn stderr(bytes: Vec<u8>) -> Self {
        Self::StdErr {
            timestamp: Timestamp::now_utc(),
            bytes,
        }
    }

    pub fn log(level: LogLevel, context: &str, message: &str) -> Self {
        Self::Log {
            timestamp: Timestamp::now_utc(),
            level,
            context: context.to_string(),
            message: message.to_string(),
        }
    }

    pub fn invocation_start(function: &str, idempotency_key: &IdempotencyKey) -> Self {
        Self::InvocationStart {
            timestamp: Timestamp::now_utc(),
            function: function.to_string(),
            idempotency_key: idempotency_key.clone(),
        }
    }

    pub fn invocation_finished(function: &str, idempotency_key: &IdempotencyKey) -> Self {
        Self::InvocationFinished {
            timestamp: Timestamp::now_utc(),
            function: function.to_string(),
            idempotency_key: idempotency_key.clone(),
        }
    }

    pub fn as_oplog_entry(&self) -> Option<OplogEntry> {
        match self {
            Self::StdOut { timestamp, bytes } => Some(OplogEntry::Log {
                timestamp: *timestamp,
                level: oplog::LogLevel::Stdout,
                context: String::new(),
                message: String::from_utf8_lossy(bytes).to_string(),
            }),
            Self::StdErr { timestamp, bytes } => Some(OplogEntry::Log {
                timestamp: *timestamp,
                level: oplog::LogLevel::Stderr,
                context: String::new(),
                message: String::from_utf8_lossy(bytes).to_string(),
            }),
            Self::Log {
                timestamp,
                level,
                context,
                message,
            } => Some(OplogEntry::Log {
                timestamp: *timestamp,
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
            }),
            Self::InvocationStart { .. } => None,
            Self::InvocationFinished { .. } => None,
        }
    }
}

impl From<InternalWorkerEvent> for WorkerEvent {
    fn from(event: InternalWorkerEvent) -> Self {
        match event {
            InternalWorkerEvent::StdOut { timestamp, bytes } =>
                Self::StdOut { timestamp, bytes },
            InternalWorkerEvent::StdErr { timestamp, bytes } =>
                Self::StdErr { timestamp, bytes },
            InternalWorkerEvent::Log { timestamp, level, context, message } =>
                Self::Log { timestamp, level, context, message },
            InternalWorkerEvent::InvocationStart { timestamp, function, idempotency_key } =>
                Self::InvocationStart { timestamp, function, idempotency_key },
            InternalWorkerEvent::InvocationFinished { timestamp, function, idempotency_key } =>
                Self::InvocationFinished { timestamp, function, idempotency_key },
        }
    }
}
