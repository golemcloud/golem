///|
/// Blocks execution until the oplog has been written to at least the
/// specified number of replicas, or the maximum number of replicas if the
/// requested number is higher.
pub fn oplog_commit(replicas : Byte) -> Unit {
  @raw.oplog_commit(replicas)
}

///|
/// Unconditionally traps the current invocation with the given reason.
/// This call never returns and fails the invocation.
/// Handle recoverable failures in guest code.
pub fn trap(reason : String) -> Unit {
  @raw.trap(reason)
}
