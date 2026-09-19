# Retry policies

The Scala retry DSL can be used in two different ways:

- `RetryApi` installs and resolves host-managed policies.
- `Retry.retry` interprets a policy locally and retries a `Future`-returning operation in user space.

For example:

```scala
import golem.host.Retry

import scala.concurrent.Future
import scala.concurrent.duration.*

final case class HttpFailure(status: Int) extends RuntimeException

val policy = Retry.Policy
  .exponential(100.millis, 2.0)
  .maxRetries(4)
  .onlyWhen(Retry.Props.statusCode.gte(500))

val result: Future[String] = Retry.retry(
  policy,
  {
    case HttpFailure(status) => List(Retry.Props.statusCode -> status)
    case _                   => Nil
  }
) {
  makeRequest()
}
```

The property function is evaluated for every failed attempt, so filtered policies can react to changing failures. A raw policy returned by `RetryApi.resolveRetryPolicy` can be passed to the same overload. Invalid or cyclic raw policy trees produce a failed `Future` before the operation starts.

For local `prop-matches` predicates, `Retry.retry` provides a small glob approximation: `*` matches zero or more characters and `?` matches one UTF-16 code unit. Every other character is literal. Matching uses an anchored JavaScript regular expression without flags, so wildcards do not match line terminators and `?` does not match an astral character such as `😀`. This is not the host's complete authoritative glob syntax; use host-driven retries when exact platform glob matching is required.

Local retries preserve the final failed `Throwable` when the policy gives up. They do not install a host policy, produce executor `RetryAttempt` oplog entries, or survive suspension/recovery as a single host-managed retry sequence. Use host-managed policies when those durable executor semantics are required.
