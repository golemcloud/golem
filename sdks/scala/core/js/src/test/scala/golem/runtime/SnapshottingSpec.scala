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

package golem.runtime

import golem.{BaseAgent, Snapshotted}
import golem.runtime.annotations.{agentDefinition, agentImplementation}
import golem.runtime.autowire.{AgentDefinition, AgentImplementation}
import zio._
import zio.test._
import zio.blocks.schema.Schema

import scala.concurrent.Future
import scala.scalajs.js

object SnapshottingSpec extends ZIOSpecDefault {

  // ---------------------------------------------------------------------------
  // 1. Custom saveSnapshot / loadSnapshot
  // ---------------------------------------------------------------------------

  @agentDefinition("custom-snapshot-agent", snapshotting = "enabled")
  trait CustomSnapshotAgent extends BaseAgent {
    class Id()
    def setValue(v: Int): Future[Unit]
    def getValue(): Future[Int]
  }

  @agentImplementation()
  final class CustomSnapshotAgentImpl() extends CustomSnapshotAgent {
    private var value: Int = 0

    def saveSnapshot(): Future[Array[Byte]] = Future.successful {
      Array(
        ((value >>> 24) & 0xff).toByte,
        ((value >>> 16) & 0xff).toByte,
        ((value >>> 8) & 0xff).toByte,
        (value & 0xff).toByte
      )
    }

    def loadSnapshot(bytes: Array[Byte]): Future[Unit] = Future.successful {
      value = ((bytes(0) & 0xff) << 24) |
        ((bytes(1) & 0xff) << 16) |
        ((bytes(2) & 0xff) << 8) |
        (bytes(3) & 0xff)
    }

    override def setValue(v: Int): Future[Unit] = Future.successful { value = v }
    override def getValue(): Future[Int]        = Future.successful(value)
  }

  private lazy val customDefn: AgentDefinition[CustomSnapshotAgent] =
    AgentImplementation.registerClass[CustomSnapshotAgent, CustomSnapshotAgentImpl]

  // ---------------------------------------------------------------------------
  // 2. Snapshotted[S] mixin
  // ---------------------------------------------------------------------------

  final case class TestState(counter: Int, label: String)
  object TestState {
    implicit val schema: Schema[TestState] = Schema.derived
  }

  @agentDefinition("auto-snapshot-agent", snapshotting = "enabled")
  trait AutoSnapshotAgent extends BaseAgent {
    class Id()
    def increment(): Future[Int]
  }

  @agentImplementation()
  final class AutoSnapshotAgentImpl() extends AutoSnapshotAgent with Snapshotted[TestState] {
    var state: TestState               = TestState(0, "initial")
    val stateSchema: Schema[TestState] = TestState.schema

    override def increment(): Future[Int] = Future.successful {
      state = state.copy(counter = state.counter + 1)
      state.counter
    }
  }

  private lazy val autoDefn: AgentDefinition[AutoSnapshotAgent] =
    AgentImplementation.registerClass[AutoSnapshotAgent, AutoSnapshotAgentImpl]

  // ---------------------------------------------------------------------------
  // 3. Agent without snapshotting (disabled)
  // ---------------------------------------------------------------------------

  @agentDefinition("no-snapshot-agent")
  trait NoSnapshotAgent extends BaseAgent {
    class Id()
    def ping(): Future[String]
  }

  @agentImplementation()
  final class NoSnapshotAgentImpl() extends NoSnapshotAgent {
    override def ping(): Future[String] = Future.successful("pong")
  }

  private lazy val noSnapDefn: AgentDefinition[NoSnapshotAgent] =
    AgentImplementation.registerClass[NoSnapshotAgent, NoSnapshotAgentImpl]

  // ---------------------------------------------------------------------------
  // Tests
  // ---------------------------------------------------------------------------

  def spec = suite("SnapshottingSpec")(
    suite("handler detection")(
      test("custom saveSnapshot/loadSnapshot generates snapshot handlers") {
        assertTrue(customDefn.snapshotHandlers.isDefined)
      },
      test("Snapshotted[S] mixin generates snapshot handlers") {
        assertTrue(autoDefn.snapshotHandlers.isDefined)
      },
      test("agent without snapshotting has no snapshot handlers") {
        assertTrue(noSnapDefn.snapshotHandlers.isEmpty)
      }
    ),
    suite("WIT metadata propagation")(
      test("enabled snapshotting agent has tag 'enabled' in agentType") {
        val tag = customDefn.agentType.snapshotting.tag
        assertTrue(tag == "enabled")
      },
      test("disabled snapshotting agent has tag 'disabled' in agentType") {
        val tag = noSnapDefn.agentType.snapshotting.tag
        assertTrue(tag == "disabled")
      }
    ),
    suite("custom snapshot roundtrip")(
      test("save produces application/octet-stream payload") {
        ZIO.fromFuture { implicit ec =>
          val instance = new CustomSnapshotAgentImpl()
          for {
            _       <- instance.setValue(42)
            payload <- customDefn.snapshotHandlers.get.save(instance)
          } yield assertTrue(
            payload.mimeType == "application/octet-stream",
            payload.bytes.nonEmpty
          )
        }
      },
      test("save/load roundtrip restores state") {
        ZIO.fromFuture { implicit ec =>
          val instance = new CustomSnapshotAgentImpl()
          for {
            _       <- instance.setValue(42)
            payload <- customDefn.snapshotHandlers.get.save(instance)
            restored = new CustomSnapshotAgentImpl()
            _       <- customDefn.snapshotHandlers.get.load(restored, payload.bytes)
            v       <- restored.getValue()
          } yield assertTrue(v == 42)
        }
      }
    ),
    suite("Snapshotted[S] roundtrip")(
      test("save produces application/json payload with state fields") {
        ZIO.fromFuture { implicit ec =>
          val instance = new AutoSnapshotAgentImpl()
          for {
            _       <- instance.increment()
            _       <- instance.increment()
            payload <- autoDefn.snapshotHandlers.get.save(instance)
          } yield {
            val json = new String(payload.bytes, "UTF-8")
            assertTrue(
              payload.mimeType == "application/json",
              json.contains("counter")
            )
          }
        }
      },
      test("save/load roundtrip restores state") {
        ZIO.fromFuture { implicit ec =>
          val instance = new AutoSnapshotAgentImpl()
          for {
            _       <- instance.increment()
            _       <- instance.increment()
            payload <- autoDefn.snapshotHandlers.get.save(instance)
            restored = new AutoSnapshotAgentImpl()
            _       <- autoDefn.snapshotHandlers.get.load(restored, payload.bytes)
            v       <- restored.increment() // counter was 2, now should be 3
          } yield assertTrue(v == 3)
        }
      }
    )
  )
}
