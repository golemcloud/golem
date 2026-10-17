/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://license.golem.cloud/LICENSE
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package golem

import golem.host.Retry
import golem.host.RetryApi
import golem.host.js.JsNamedRetryPolicy

import scala.concurrent.{ExecutionContext, Future}

/**
 * Utility guards that mirror the ergonomics of the JS SDK's guard helpers.
 *
 * Each `use*` method applies a configuration change on the host and returns a
 * guard that will automatically restore the previous value when `drop()` (or
 * `close()`) is invoked. The `with*` variants execute the supplied block with
 * the new setting and guarantee restoration after the returned `Future`
 * completes.
 */
/** Scoped runtime controls for Scala.js agents. */
object Guards {
  def withRetryPolicy[A](policy: JsNamedRetryPolicy)(block: => Future[A]): Future[A] =
    withGuard(useRetryPolicy(policy))(block)

  def withRetryPolicy[A](policy: Retry.NamedPolicy)(block: => Future[A]): Future[A] =
    withRetryPolicy(Retry.namedPolicyToJsOrThrow(policy))(block)

  def useRetryPolicy(policy: JsNamedRetryPolicy): RetryPolicyGuard = {
    val previous = RetryApi.getRetryPolicyByName(policy.name)
    val name     = policy.name
    RetryApi.setRetryPolicy(policy)
    new RetryPolicyGuard(() =>
      previous match {
        case Some(original) => RetryApi.setRetryPolicy(original)
        case None           => RetryApi.removeRetryPolicy(name)
      }
    )
  }

  def useRetryPolicy(policy: Retry.NamedPolicy): RetryPolicyGuard =
    useRetryPolicy(Retry.namedPolicyToJsOrThrow(policy))

  private implicit val ec: ExecutionContext = ExecutionContext.global

  private def withGuard[A, G <: Guard](guard: => G)(block: => Future[A]): Future[A] = {
    val active = guard
    try block.andThen { case _ => active.drop() }
    catch {
      case error: Throwable =>
        active.drop()
        throw error
    }
  }

  sealed abstract class Guard private[golem] (release: () => Unit) extends AutoCloseable {
    private var active = true

    final override def close(): Unit = drop()

    final def drop(): Unit =
      if (active) {
        active = false
        release()
      }
  }

  final class RetryPolicyGuard private[golem] (release: () => Unit) extends Guard(release)
}
