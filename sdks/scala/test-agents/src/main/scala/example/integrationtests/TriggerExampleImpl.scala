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

package example.integrationtests

import golem.Datetime
import golem.runtime.annotations.agentImplementation
import golem.runtime.rpc.{AgentRpcException, Invocation, InvocationStatus}

import golem.concurrent.Implicits.global

import scala.annotation.unused
import scala.concurrent.{Future, Promise}
import scala.scalajs.js.timers
import scala.util.{Failure, Success}

@agentImplementation()
final class TriggerTargetImpl(@unused private val name: String) extends TriggerTarget {
  private var count: Int = 0

  override def process(x: Int, label: String): Future[Int] =
    Future.successful {
      count += x
      count
    }

  override def ping(): Future[String] =
    Future.successful("pong")

  override def slowCount(millis: Int): Future[Int] = {
    val done = Promise[Int]()
    timers.setTimeout(millis.toDouble)(done.success(count))
    done.future
  }
}

@agentImplementation()
final class TriggerCallerImpl(private val name: String) extends TriggerCaller {
  private val target = TriggerTargetClient.upsert("target-instance")

  override def fireProcess(): Future[String] =
    target.process
      .trigger(42, "hello")
      .map(invocation => s"triggered process(42, \"hello\") on target-instance as ${invocation.id}")

  override def firePing(): Future[String] =
    target.ping.trigger().map(invocation => s"triggered ping() on target-instance as ${invocation.id}")

  override def inspectHandles(): Future[String] = {
    val handles = TriggerTargetClient.upsert(s"$name-handles")
    for {
      triggered       <- handles.process.trigger(5, "handles")
      processed       <- triggered.awaitResult()
      triggeredStatus <- triggered.status()
      cancelCompleted <- triggered.cancel()
      pinged          <- handles.ping.trigger()
      rebuiltPing     <- Invocation.get[String](pinged.id).awaitResult()
      scheduled       <- handles.process.scheduleAt(100, "later", Datetime.afterSeconds(3600))
      scheduledStatus <- scheduled.status()
      cancelScheduled <- scheduled.cancel()
      cancelledStatus <- scheduled.status()
      awaitCancelled  <- errorName(scheduled.awaitResult())
      slow            <- handles.slowCount.trigger(5000)
      runningStatus   <- waitUntilStarted(slow, 100)
      cancelRunning   <- errorName(slow.cancel())
      slowResult      <- slow.awaitResult()
    } yield List(
      triggeredStatus,
      cancelCompleted,
      processed,
      rebuiltPing,
      scheduledStatus,
      cancelScheduled,
      cancelledStatus,
      awaitCancelled,
      runningStatus,
      cancelRunning,
      slowResult
    ).mkString(":")
  }

  private def waitUntilStarted(invocation: Invocation[?], attempts: Int): Future[InvocationStatus] =
    invocation.status().flatMap {
      case InvocationStatus.Pending if attempts > 0 =>
        val delay = Promise[Unit]()
        timers.setTimeout(100)(delay.success(()))
        delay.future.flatMap(_ => waitUntilStarted(invocation, attempts - 1))
      case status => Future.successful(status)
    }

  private def errorName(operation: Future[?]): Future[String] =
    operation.transform {
      case Success(value)                    => Success(s"unexpected:$value")
      case Failure(AgentRpcException(error)) => Success(error.getClass.getSimpleName)
      case Failure(other)                    => Success(s"unexpected:error:$other")
    }
}
