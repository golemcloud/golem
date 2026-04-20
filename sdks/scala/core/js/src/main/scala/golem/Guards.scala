/*
 * Copyright 2024-2026 John A. De Goes and the ZIO Contributors
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package golem

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
  def withPersistenceLevel[A](level: HostApi.PersistenceLevel)(block: => Future[A]): Future[A] =
    withGuard(usePersistenceLevel(level))(block)

  def usePersistenceLevel(level: HostApi.PersistenceLevel): PersistenceLevelGuard = {
    val original = HostApi.getOplogPersistenceLevel()
    HostApi.setOplogPersistenceLevel(level)
    new PersistenceLevelGuard(() => HostApi.setOplogPersistenceLevel(original))
  }

  def withRetryPolicy[A](policy: JsNamedRetryPolicy)(block: => Future[A]): Future[A] =
    withGuard(useRetryPolicy(policy))(block)

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

  def withIdempotenceMode[A](flag: Boolean)(block: => Future[A]): Future[A] =
    withGuard(useIdempotenceMode(flag))(block)

  def useIdempotenceMode(flag: Boolean): IdempotenceModeGuard = {
    val original = HostApi.getIdempotenceMode()
    HostApi.setIdempotenceMode(flag)
    new IdempotenceModeGuard(() => HostApi.setIdempotenceMode(original))
  }

  /**
   * Executes a block atomically.
   *
   * On success the atomic region is committed via `markEndOperation`. On
   * failure the region is intentionally left open so the executor sees the trap
   * as occurring *inside* the atomic region and can retry from the
   * begin-operation marker (matching the Rust SDK behaviour where `Drop` is
   * never called on panic because WASM panics don't unwind).
   */
  def atomically[A](block: => Future[A]): Future[A] = {
    val guard = markAtomicOperation()
    block.transform(
      result => { guard.drop(); result },
      error => { /* Do NOT drop — leave the atomic region open for retry */
        error
      }
    )
  }

  def markAtomicOperation(): AtomicOperationGuard = {
    val begin = HostApi.markBeginOperation()
    new AtomicOperationGuard(() => HostApi.markEndOperation(begin))
  }

  private implicit val ec: ExecutionContext = ExecutionContext.global

  private def withGuard[A, G <: Guard](guard: => G)(block: => Future[A]): Future[A] = {
    val active = guard
    block.andThen { case _ => active.drop() }
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

  final class PersistenceLevelGuard private[golem] (release: () => Unit) extends Guard(release)

  final class RetryPolicyGuard private[golem] (release: () => Unit) extends Guard(release)

  final class IdempotenceModeGuard private[golem] (release: () => Unit) extends Guard(release)

  final class AtomicOperationGuard private[golem] (release: () => Unit) extends Guard(release)
}
