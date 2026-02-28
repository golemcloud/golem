trait Logger {
  write_string(Self, String) -> Unit
}

impl Logger for StringBuilder with write_string(self, str) {
  self.write_string(str)
}

trait Loggable {
  log(Self, &Logger) -> Unit
}

fn[X : Loggable] &Logger::write_obj(self : &Logger, x : X) -> Unit {
  x.log(self)
}

fn[X : Loggable] &Logger::write_obj_ln(self : &Logger, x : X) -> Unit {
  x.log(self)
  self.write_string("\n")
}

impl Loggable for Int with log(self, logger) { logger.write_string(self.to_string()) }

impl[X : Loggable, Y : Loggable] Loggable for (X, Y) with log(self, logger) {
  logger
  ..write_string("(")
  ..write_obj(self.0)
  ..write_string(", ")
  ..write_obj(self.1)
  ..write_string(")")
}

fn init {
  let buf = StringBuilder::new()
  (buf as &Logger)
  ..write_string("=== start logging ===\n")
  ..write_obj_ln(42)
  ..write_obj_ln((1, (2, (3, 4))))
  &Logger::write_obj_ln(buf, 0)
  &Logger::write_string(buf, "=== end logging ===")
  println(buf.to_string())
}
