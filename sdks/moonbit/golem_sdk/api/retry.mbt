///|
pub using @raw {type RetryPolicy}

///|
/// Gets the current retry policy associated with the agent.
pub fn get_retry_policy() -> RetryPolicy {
  @raw.get_retry_policy()
}

///|
/// Overrides the current retry policy associated with the agent.
pub fn set_retry_policy(policy : RetryPolicy) -> Unit {
  @raw.set_retry_policy(policy)
}

///|
/// Runs a function with the given retry policy, restoring the previous
/// policy afterward (even if the function raises).
pub fn[T] with_retry_policy(policy : RetryPolicy, f : () -> T) -> T {
  let prev = @raw.get_retry_policy()
  @raw.set_retry_policy(policy)
  defer @raw.set_retry_policy(prev)
  f()
}
