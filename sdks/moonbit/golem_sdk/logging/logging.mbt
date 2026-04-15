///|
pub using @wasi_logging {type Level}

///|
priv struct LoggingState {
  mut context : String
  mut min_level : Level
}

///|
let state : LoggingState = { context: "", min_level: TRACE }

///|
/// Sets the global default context string used by module-level logging functions.
pub fn set_context(context : String) -> Unit {
  state.context = context
}

///|
/// Sets the global minimum log level. Messages below this level are suppressed.
/// Defaults to `INFO`.
pub fn set_min_level(level : Level) -> Unit {
  state.min_level = level
}

///|
/// A lightweight logger carrying a context string and optional minimum level.
/// Immutable value type — `with_name` and `with_min_level` return new loggers.
pub(all) struct Logger {
  context : String
  min_level : Level?
}

///|
/// Creates a new logger with the given context and optional minimum level.
pub fn Logger::new(
  context? : String = "",
  min_level? : Level? = None,
) -> Logger {
  { context, min_level }
}

///|
/// Returns a new logger with the given name segment appended to the context,
/// separated by `/`.
pub fn Logger::with_name(self : Logger, name : String) -> Logger {
  if name.is_empty() {
    return self
  }
  let new_context = if self.context.is_empty() {
    name
  } else {
    self.context + "/" + name
  }
  { ..self, context: new_context }
}

///|
/// Returns a new logger with the given minimum level override.
pub fn Logger::with_min_level(self : Logger, level : Level) -> Logger {
  { ..self, min_level: Some(level) }
}

///|
fn Logger::effective_min_level(self : Logger) -> Level {
  match self.min_level {
    Some(level) => level
    None => state.min_level
  }
}

///|
fn enabled(level : Level, min_level : Level) -> Bool {
  level.ordinal() >= min_level.ordinal()
}

///|
/// Logs a message at the given level using this logger's context.
pub fn Logger::log(self : Logger, level : Level, message : String) -> Unit {
  if enabled(level, self.effective_min_level()) {
    @wasi_logging.log(level, self.context, message)
  }
}

///|
pub fn Logger::trace(self : Logger, message : String) -> Unit {
  self.log(TRACE, message)
}

///|
pub fn Logger::debug(self : Logger, message : String) -> Unit {
  self.log(DEBUG, message)
}

///|
pub fn Logger::info(self : Logger, message : String) -> Unit {
  self.log(INFO, message)
}

///|
pub fn Logger::warn(self : Logger, message : String) -> Unit {
  self.log(WARN, message)
}

///|
pub fn Logger::error(self : Logger, message : String) -> Unit {
  self.log(ERROR, message)
}

///|
pub fn Logger::critical(self : Logger, message : String) -> Unit {
  self.log(CRITICAL, message)
}

///|
/// Creates a new logger from the global default context with the given name
/// segment appended.
pub fn with_name(name : String) -> Logger {
  Logger::new(context=state.context).with_name(name)
}

///|
/// Logs a message at the given level using the global context.
pub fn log(level : Level, message : String) -> Unit {
  if enabled(level, state.min_level) {
    @wasi_logging.log(level, state.context, message)
  }
}

///|
pub fn trace(message : String) -> Unit {
  log(TRACE, message)
}

///|
pub fn debug(message : String) -> Unit {
  log(DEBUG, message)
}

///|
pub fn info(message : String) -> Unit {
  log(INFO, message)
}

///|
pub fn warn(message : String) -> Unit {
  log(WARN, message)
}

///|
pub fn error(message : String) -> Unit {
  log(ERROR, message)
}

///|
pub fn critical(message : String) -> Unit {
  log(CRITICAL, message)
}
