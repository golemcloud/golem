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

import scala.concurrent.{ExecutionContext, Future}

/**
 * A checkpoint that captures the current oplog index and can revert execution
 * to that point.
 *
 * Use [[Checkpoint.apply]] to create a new checkpoint, or [[Checkpoint.withCheckpoint]]
 * / [[Checkpoint.withCheckpointTry]] to execute a block with automatic revert on failure.
 */
final class Checkpoint private (private val oplogIndex: HostApi.OplogIndex) {

  private implicit val ec: ExecutionContext = ExecutionContext.global

  /**
   * Reverts execution to the oplog index captured when this checkpoint was
   * created. This method never returns normally.
   */
  def revert(): Nothing = {
    HostApi.setOplogIndex(oplogIndex)
    throw new AssertionError("Unreachable: reverted to checkpoint")
  }

  /**
   * Returns the successful value, or reverts to the checkpoint if the result
   * is a `Left`.
   */
  def unwrapOrRevert[T](result: Either[_, T]): T =
    result match {
      case Right(value) => value
      case Left(_)      => revert()
    }

  /**
   * Runs the given function that returns a `Future[Either]`, reverting to the
   * checkpoint if the `Either` is `Left`.
   */
  def runOrRevert[T](fn: => Future[Either[_, T]]): Future[T] =
    fn.map(unwrapOrRevert(_))

  /**
   * Executes the given block that returns a `Future`. If the `Future` fails,
   * reverts to the checkpoint.
   */
  def tryOrRevert[T](fn: => Future[T]): Future[T] =
    fn.recover { case _ => revert() }

  /**
   * Asserts a condition. If the condition is false, reverts to the checkpoint.
   */
  def assertOrRevert(condition: Boolean): Unit =
    if (!condition) revert()
}

object Checkpoint {

  private implicit val ec: ExecutionContext = ExecutionContext.global

  /** Creates a new checkpoint at the current oplog index. */
  def apply(): Checkpoint =
    new Checkpoint(HostApi.getOplogIndex())

  /**
   * Creates a checkpoint and executes the given function. If the function
   * returns a `Left`, reverts to the checkpoint.
   *
   * @return A `Future` of the successful value.
   */
  def withCheckpoint[T](fn: Checkpoint => Future[Either[_, T]]): Future[T] = {
    val cp = Checkpoint()
    fn(cp).map(cp.unwrapOrRevert(_))
  }

  /**
   * Creates a checkpoint and executes the given function. If the `Future`
   * fails, reverts to the checkpoint.
   *
   * @return A `Future` of the result.
   */
  def withCheckpointTry[T](fn: Checkpoint => Future[T]): Future[T] = {
    val cp = Checkpoint()
    fn(cp).recover { case _ => cp.revert() }
  }
}
