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
 * Each `use*` method opens a retry policy scope on the host and returns a guard
 * that closes it when `drop()` (or `close()`) is invoked. While the scope is
 * open, its named policy overrides the agent-wide policy with the same name for
 * the host calls of the current task; the agent-wide policies do not change and
 * no oplog entry is recorded. JavaScript code of one agent runs on one task, so
 * other async work of the same invocation that runs while the scope is open
 * also sees the policy. The `with*` variants execute the supplied block inside
 * the scope and close it after the returned `Future` completes.
 */
/** Scoped runtime controls for Scala.js agents. */
object Guards {
  def withRetryPolicy[A](policy: JsNamedRetryPolicy)(block: => Future[A]): Future[A] =
    withGuard(useRetryPolicy(policy))(block)

  def withRetryPolicy[A](policy: Retry.NamedPolicy)(block: => Future[A]): Future[A] =
    withRetryPolicy(Retry.namedPolicyToJsOrThrow(policy))(block)

  def useRetryPolicy(policy: JsNamedRetryPolicy): RetryPolicyGuard =
    new RetryPolicyGuard(RetryApi.openRetryPolicyScope(policy))

  def useRetryPolicy(policy: Retry.NamedPolicy): RetryPolicyGuard =
    useRetryPolicy(Retry.namedPolicyToJsOrThrow(policy))

  private implicit val ec: ExecutionContext = golem.concurrent.GolemExecutionContext.global

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
