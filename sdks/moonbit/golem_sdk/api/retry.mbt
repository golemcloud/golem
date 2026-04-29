///|
pub using @retry {type NamedRetryPolicy}

///|
pub using @retry {type RetryPolicy}

///|
pub using @retry {type PredicateValue}

///|
/// Gets all retry policies active for this agent.
pub fn get_retry_policies() -> Array[NamedRetryPolicy] {
  @retry.get_retry_policies()
}

///|
/// Gets a specific retry policy by name.
pub fn get_retry_policy_by_name(name : String) -> NamedRetryPolicy? {
  @retry.get_retry_policy_by_name(name)
}

///|
/// Resolves the matching retry policy for a given operation context.
/// Evaluates named policies in descending priority order; returns the
/// policy from the first rule whose predicate matches, or None.
pub fn resolve_retry_policy(
  verb : String,
  noun_uri : String,
  properties : Array[(String, PredicateValue)],
) -> RetryPolicy? {
  @retry.resolve_retry_policy(verb, noun_uri, properties)
}

///|
/// Add or overwrite a named retry policy (persisted to oplog).
/// If a policy with the same name exists, it is replaced.
pub fn set_retry_policy(policy : NamedRetryPolicy) -> Unit {
  @retry.set_retry_policy(policy)
}

///|
/// Add or overwrite a high-level named retry policy after validating and
/// compiling it to the raw flattened WIT representation.
pub fn set_named_policy(policy : NamedPolicy) -> Unit raise RetryBuilderError {
  set_retry_policy(policy.to_raw())
}

///|
/// Remove a named retry policy by name (persisted to oplog).
pub fn remove_retry_policy(name : String) -> Unit {
  @retry.remove_retry_policy(name)
}

///|
/// Temporarily sets a named retry policy. When the returned guard is dropped,
/// the previous policy with the same name is restored (or removed if it
/// didn't exist before).
pub fn use_retry_policy(policy : NamedRetryPolicy) -> RetryPolicyGuard {
  let previous = @retry.get_retry_policy_by_name(policy.name)
  let name = policy.name
  @retry.set_retry_policy(policy)
  { previous, name }
}

///|
/// Temporarily sets a high-level named retry policy. The previous raw policy
/// with the same name is restored when the returned guard is dropped.
pub fn use_named_policy(
  policy : NamedPolicy,
) -> RetryPolicyGuard raise RetryBuilderError {
  use_retry_policy(policy.to_raw())
}

///|
/// Runs a function with a named retry policy temporarily set.
/// The previous policy is restored (or removed) after the function completes,
/// even if it raises.
pub fn[T] with_retry_policy(policy : NamedRetryPolicy, f : () -> T) -> T {
  let g = use_retry_policy(policy)
  defer g.drop()
  f()
}

///|
/// Runs a function with a high-level named retry policy temporarily set.
/// The previous raw policy is restored even if the function raises.
pub fn[T] with_named_policy(
  policy : NamedPolicy,
  f : () -> T,
) -> T raise RetryBuilderError {
  let g = use_named_policy(policy)
  defer g.drop()
  f()
}

///|
/// Guard that restores the previous state of a named retry policy on drop.
/// If the policy existed before, it is restored; if it was newly added, it
/// is removed.
pub(all) struct RetryPolicyGuard {
  previous : NamedRetryPolicy?
  name : String
}

///|
/// Restores the previous retry policy (or removes it if it was newly added).
pub fn RetryPolicyGuard::drop(self : RetryPolicyGuard) -> Unit {
  match self.previous {
    Some(original) => @retry.set_retry_policy(original)
    None => @retry.remove_retry_policy(self.name)
  }
}
