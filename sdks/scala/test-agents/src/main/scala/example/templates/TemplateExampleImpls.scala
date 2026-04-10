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

package example.templates

import golem._
import golem.runtime.annotations.agentImplementation

import scala.annotation.unused
import scala.concurrent.Future
import scala.scalajs.concurrent.JSExecutionContext.Implicits.queue
import scala.scalajs.js

@agentImplementation()
final class CounterImpl(@unused private val name: String) extends Counter {
  private var value: Int = 0

  override def increment(): Future[Int] =
    Future.successful {
      value += 1
      value
    }
}

@agentImplementation()
final class RpcClientImpl(@unused private val name: String) extends RpcClient {
  override def callCounter(counterId: String): Future[Int] =
    CounterClient.get(counterId).increment()
}

@agentImplementation()
final class TasksImpl(@unused private val name: String) extends Tasks {
  private var nextId: Int       = 1
  private var tasks: List[Task] = Nil

  override def createTask(request: CreateTaskRequest): Future[Task] =
    Future.successful {
      val task = Task(
        id = nextId,
        title = request.title,
        completed = false,
        createdAt = new js.Date().toISOString()
      )
      nextId += 1
      tasks = tasks :+ task
      task
    }

  override def getTasks(): Future[List[Task]] =
    Future.successful(tasks)

  override def completeTask(id: Int): Future[Option[Task]] =
    Future.successful {
      tasks.find(_.id == id).map { t =>
        val updated = t.copy(completed = true)
        tasks = tasks.map(curr => if (curr.id == id) updated else curr)
        updated
      }
    }
}

@agentImplementation()
final class SnapshotCounterImpl(@unused private val name: String) extends SnapshotCounter {
  private var value: Int = 0

  def saveSnapshot(): Future[Array[Byte]] =
    Future.successful(encodeU32(value))

  def loadSnapshot(bytes: Array[Byte]): Future[Unit] =
    Future.successful {
      value = decodeU32(bytes)
    }

  override def increment(): Future[Int] =
    Future.successful {
      value += 1
      value
    }

  private def encodeU32(i: Int): Array[Byte] =
    Array(
      ((i >>> 24) & 0xff).toByte,
      ((i >>> 16) & 0xff).toByte,
      ((i >>> 8) & 0xff).toByte,
      (i & 0xff).toByte
    )

  private def decodeU32(bytes: Array[Byte]): Int =
    ((bytes(0) & 0xff) << 24) |
      ((bytes(1) & 0xff) << 16) |
      ((bytes(2) & 0xff) << 8) |
      (bytes(3) & 0xff)
}

final case class SnapshotCounterState(value: Int)
object SnapshotCounterState {
  implicit val schema: zio.blocks.schema.Schema[SnapshotCounterState] = zio.blocks.schema.Schema.derived
}

@agentImplementation()
final class AutoSnapshotCounterImpl(@unused private val name: String)
    extends AutoSnapshotCounter
    with Snapshotted[SnapshotCounterState] {

  var state: SnapshotCounterState                                 = SnapshotCounterState(0)
  val stateSchema: zio.blocks.schema.Schema[SnapshotCounterState] = SnapshotCounterState.schema

  override def increment(): Future[Int] =
    Future.successful {
      state = state.copy(value = state.value + 1)
      state.value
    }
}

@agentImplementation()
final class ApprovalWorkflowImpl(private val workflowId: String) extends ApprovalWorkflow {
  private var promiseId: Option[HostApi.PromiseId] = None
  private var decided: Option[String]              = None

  override def begin(): Future[String] =
    Future.successful {
      promiseId match {
        case Some(_) =>
          s"workflow $workflowId already pending"
        case None =>
          promiseId = Some(HostApi.createPromise())
          decided = None
          s"workflow $workflowId pending"
      }
    }

  override def awaitOutcome(): Future[String] =
    decided match {
      case Some(value) =>
        Future.successful(value)
      case None =>
        promiseId match {
          case None =>
            Future.successful("no pending approval")
          case Some(p) =>
            HostApi.awaitPromise(p).map { bytes =>
              val v = Utf8.decodeBytes(bytes)
              decided = Some(v)
              v
            }
        }
    }

  override def complete(decision: String): Future[Boolean] =
    promiseId match {
      case None =>
        Future.successful(false)
      case Some(p) =>
        Future.successful {
          decided = Some(decision)
          val ok = HostApi.completePromise(p, Utf8.encodeBytes(decision))
          ok
        }
    }
}

@agentImplementation()
final class HumanAgentImpl(private val username: String) extends HumanAgent {
  override def decide(workflowId: String, decision: String): Future[String] =
    ApprovalWorkflowClient
      .get(workflowId)
      .complete(decision)
      .map { ok =>
        if (ok) s"$username decided $decision for $workflowId"
        else s"$username failed to decide for $workflowId"
      }
}
