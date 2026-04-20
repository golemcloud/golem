///|
pub using @raw {type PersistenceLevel}

///|
/// Returns the current position in the persistent op log.
pub fn get_oplog_index() -> UInt64 {
  @raw.get_oplog_index()
}

///|
/// Makes the current agent travel back in time and continue execution from
/// the given position in the persistent op log.
pub fn set_oplog_index(oplog_idx : UInt64) -> Unit {
  @raw.set_oplog_index(oplog_idx)
}

///|
/// Blocks the execution until the oplog has been written to at least the
/// specified number of replicas, or the maximum number of replicas if the
/// requested number is higher.
pub fn oplog_commit(replicas : Byte) -> Unit {
  @raw.oplog_commit(replicas)
}

///|
/// Gets the agent's current persistence level.
pub fn get_oplog_persistence_level() -> PersistenceLevel {
  @raw.get_oplog_persistence_level()
}

///|
/// Sets the agent's current persistence level. This can increase the
/// performance of execution in cases where durable execution is not required.
pub fn set_oplog_persistence_level(level : PersistenceLevel) -> Unit {
  @raw.set_oplog_persistence_level(level)
}

///|
/// Runs a function with the given persistence level, restoring the previous
/// level afterward (even if the function raises).
pub fn[T] with_persistence_level(level : PersistenceLevel, f : () -> T) -> T {
  let prev = @raw.get_oplog_persistence_level()
  @raw.set_oplog_persistence_level(level)
  defer @raw.set_oplog_persistence_level(prev)
  f()
}

///|
/// Marks the beginning of an atomic operation.
/// In case of a failure within the region, the whole region will be
/// reexecuted on retry. Use `with_atomic_operation` for automatic lifecycle.
pub fn mark_begin_operation() -> UInt64 {
  @raw.mark_begin_operation()
}

///|
/// Commits an atomic operation. After calling this for a given index,
/// further calls with the same parameter will do nothing.
pub fn mark_end_operation(begin : UInt64) -> Unit {
  @raw.mark_end_operation(begin)
}

///|
/// Runs a function within an atomic operation region.
/// The region is automatically committed when `f` returns, even if it raises.
pub fn[T] with_atomic_operation(f : () -> T) -> T {
  let marker = @raw.mark_begin_operation()
  defer @raw.mark_end_operation(marker)
  f()
}
