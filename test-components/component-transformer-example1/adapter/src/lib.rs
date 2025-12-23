use bindings::exports::wasi::logging::logging::{Guest, Level};

#[allow(warnings)]
#[allow(static_mut_refs)]
mod bindings;

struct Component;

impl Guest for Component {
    fn log(
        level: bindings::exports::wasi::logging::logging::Level,
        context: String,
        message: String,
    ) {
        let new_context = format!("custom-context");
        let updated_context = if context.is_empty() {
            new_context
        } else {
            format!("{context},custom-context")
        };
        bindings::wasi::logging::logging::log(level.into(), &updated_context, &message);
    }
}

impl From<Level> for bindings::wasi::logging::logging::Level {
    fn from(value: Level) -> Self {
        match value {
            Level::Trace => bindings::wasi::logging::logging::Level::Trace,
            Level::Debug => bindings::wasi::logging::logging::Level::Debug,
            Level::Info => bindings::wasi::logging::logging::Level::Info,
            Level::Warn => bindings::wasi::logging::logging::Level::Warn,
            Level::Error => bindings::wasi::logging::logging::Level::Error,
            Level::Critical => bindings::wasi::logging::logging::Level::Critical,
        }
    }
}

bindings::export!(Component with_types_in bindings);
