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

package golem.runtime

import golem.{BaseAgent, FutureInterop, Principal, Snapshotted}
import golem.config.{AgentConfig, Config, ConfigHolder}
import golem.host.js.{JsSnapshot, PrincipalConverter}
import golem.runtime.annotations.{agentDefinition, agentImplementation}
import golem.runtime.autowire.{AgentDefinition, AgentImplementation, SchemaPayload}
import golem.runtime.guest.Guest
import zio._
import zio.test._
import zio.blocks.schema.Schema

import scala.concurrent.Future
import scala.scalajs.js
import scala.scalajs.js.typedarray.Uint8Array

object SnapshottingSpec extends ZIOSpecDefault {

  final case class RestoreConfig(endpoint: String)
  object RestoreConfig {
    implicit val schema: Schema[RestoreConfig] = Schema.derived
  }

  // ---------------------------------------------------------------------------
  // 1. Custom saveSnapshot / loadSnapshot
  // ---------------------------------------------------------------------------

  @agentDefinition("custom-snapshot-agent", snapshotting = "enabled")
  trait CustomSnapshotAgent extends BaseAgent {
    class Id(val name: String)
    def setValue(v: Int): Future[Unit]
    def getValue(): Future[Int]
  }

  @agentImplementation()
  final class CustomSnapshotAgentImpl(name: String) extends CustomSnapshotAgent {
    if (name == "fresh") CustomSnapshotAgentImpl.normalInitializationCount += 1

    private var value: Int                                       = 0
    private[runtime] var restoredContext: SnapshotRestoreContext = null

    def saveSnapshot(): Future[Array[Byte]] = Future.successful {
      Array(
        ((value >>> 24) & 0xff).toByte,
        ((value >>> 16) & 0xff).toByte,
        ((value >>> 8) & 0xff).toByte,
        (value & 0xff).toByte
      )
    }

    private[runtime] def restoreValue(restored: Int): Unit = value = restored

    override def setValue(v: Int): Future[Unit] = Future.successful { value = v }
    override def getValue(): Future[Int]        = Future.successful(value)
  }

  object CustomSnapshotAgentImpl {
    var normalInitializationCount: Int = 0

    def loadSnapshot(bytes: Array[Byte], context: SnapshotRestoreContext): Future[CustomSnapshotAgentImpl] =
      Future.successful {
        val value = ((bytes(0) & 0xff) << 24) |
          ((bytes(1) & 0xff) << 16) |
          ((bytes(2) & 0xff) << 8) |
          (bytes(3) & 0xff)
        val instance = new CustomSnapshotAgentImpl(s"restored:${context.identity[String](0)}")
        instance.restoredContext = context
        instance.restoreValue(value)
        instance
      }
  }

  private lazy val customDefn: AgentDefinition[CustomSnapshotAgent] =
    AgentImplementation.registerClass[CustomSnapshotAgent, CustomSnapshotAgentImpl]

  @agentDefinition("config-snapshot-agent", snapshotting = "enabled")
  trait ConfigSnapshotAgent extends BaseAgent with AgentConfig[RestoreConfig] {
    class Id()
  }

  @agentImplementation()
  final class ConfigSnapshotAgentImpl() extends ConfigSnapshotAgent {
    def saveSnapshot(): Future[Array[Byte]] = Future.successful(Array.emptyByteArray)
  }

  object ConfigSnapshotAgentImpl {
    var restoredConfig: Option[Config[RestoreConfig]] = None

    def loadSnapshot(
      bytes: Array[Byte],
      context: SnapshotRestoreContext
    ): Future[ConfigSnapshotAgentImpl] = {
      restoredConfig = Some(context.config[RestoreConfig])
      if (bytes.nonEmpty) Future.failed(new IllegalArgumentException("expected an empty snapshot"))
      else Future.successful(new ConfigSnapshotAgentImpl())
    }
  }

  private lazy val configDefn: AgentDefinition[ConfigSnapshotAgent] =
    AgentImplementation.registerClass[ConfigSnapshotAgent, ConfigSnapshotAgentImpl]

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
    var state: TestState = TestState(0, "initial")

    override def increment(): Future[Int] = Future.successful {
      state = state.copy(counter = state.counter + 1)
      state.counter
    }
  }

  object AutoSnapshotAgentImpl {
    def loadSnapshot(state: TestState, context: SnapshotRestoreContext): Future[AutoSnapshotAgentImpl] =
      Future.successful {
        val instance = new AutoSnapshotAgentImpl()
        instance.state = state
        instance
      }
  }

  private val restoreContext =
    SnapshotRestoreContext(Vector("identity"), "test-agent", None, Principal.Anonymous, None)

  private lazy val autoDefn: AgentDefinition[AutoSnapshotAgent] =
    AgentImplementation.registerClass[AutoSnapshotAgent, AutoSnapshotAgentImpl]

  private def toUint8Array(bytes: Array[Byte]): Uint8Array = {
    val result = new Uint8Array(bytes.length)
    bytes.indices.foreach(index => result(index) = bytes(index))
    result
  }

  private def fromUint8Array(bytes: Uint8Array): Array[Byte] =
    Array.tabulate(bytes.length)(index => bytes(index).toByte)

  private def binaryEnvelope(principal: Principal, state: Array[Byte]): Array[Byte] = {
    val principalBytes = PrincipalConverter.toJson(principal)
    val result         = new Array[Byte](5 + principalBytes.length + state.length)
    result(0) = 2
    result(1) = ((principalBytes.length >>> 24) & 0xff).toByte
    result(2) = ((principalBytes.length >>> 16) & 0xff).toByte
    result(3) = ((principalBytes.length >>> 8) & 0xff).toByte
    result(4) = (principalBytes.length & 0xff).toByte
    java.lang.System.arraycopy(principalBytes, 0, result, 5, principalBytes.length)
    java.lang.System.arraycopy(state, 0, result, 5 + principalBytes.length, state.length)
    result
  }

  private def jsonEnvelope(principal: Principal, state: String): Array[Byte] = {
    val principalJson = new String(PrincipalConverter.toJson(principal), "UTF-8")
    s"""{"version":1,"principal":$principalJson,"state":$state}""".getBytes("UTF-8")
  }

  private def installTestHost(agentId: String, typeName: String, identity: js.Any): Unit = {
    val globalThis = js.Dynamic.global.selectDynamic("globalThis")
    globalThis.updateDynamic("__golemScalaTestHost")(
      js.Dynamic.literal(
        "getSelfMetadata" -> (() => js.Dynamic.literal("agentId" -> js.Dynamic.literal("agentId" -> agentId))),
        "parseAgentId"    -> ((_: String) =>
          js.Array[js.Any](
            typeName,
            js.Dynamic.literal("value" -> identity),
            js.undefined.asInstanceOf[js.Any]
          )
        )
      )
    )
  }

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
          val instance = new CustomSnapshotAgentImpl("direct")
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
          val instance = new CustomSnapshotAgentImpl("direct")
          for {
            _        <- instance.setValue(42)
            payload  <- customDefn.snapshotHandlers.get.save(instance)
            restored <- customDefn.snapshotHandlers.get.load(payload.bytes, restoreContext)
            v        <- restored.getValue()
            resaved  <- customDefn.snapshotHandlers.get.save(restored)
          } yield assertTrue(v == 42, resaved.bytes.toSeq == payload.bytes.toSeq)
        }
      },
      test("restore receives identity, agent ID, phantom ID, and restored principal") {
        ZIO.fromFuture { implicit ec =>
          val phantom = golem.Uuid(BigInt(1), BigInt(2))
          val context = SnapshotRestoreContext(
            Vector("identity"),
            "custom-snapshot-agent(\"identity\")",
            Some(phantom),
            Principal.GolemUser(golem.Uuid(BigInt(3), BigInt(4))),
            None
          )
          customDefn.snapshotHandlers.get.load(Array[Byte](0, 0, 0, 42), context).map { restored =>
            val received = restored.asInstanceOf[CustomSnapshotAgentImpl].restoredContext
            assertTrue(
              received.identity[String](0) == "identity",
              received.agentId == context.agentId,
              received.phantomId == Some(phantom),
              received.restoredPrincipal == context.restoredPrincipal
            )
          }
        }
      },
      test("erased restoration decodes context without invoking the normal initialization path") {
        val identity = SchemaPayload.encode[String]("fresh")(InputRecordCodec.single[String]("name"))
        ZIO.fromFuture { implicit ec =>
          for {
            _         <- FutureInterop.fromPromise(customDefn.initialize(identity, Principal.Anonymous))
            freshCount = CustomSnapshotAgentImpl.normalInitializationCount
            _          = CustomSnapshotAgentImpl.normalInitializationCount = 0
            restored  <- FutureInterop.fromPromise(
                          customDefn.restoreAny(
                            Array[Byte](0, 0, 0, 42),
                            "custom-snapshot-agent-id",
                            identity,
                            None,
                            Principal.Anonymous
                          )
                        )
            restoredValue <- restored.instance.asInstanceOf[CustomSnapshotAgent].getValue()
            received       = restored.instance.asInstanceOf[CustomSnapshotAgentImpl].restoredContext
          } yield assertTrue(
            freshCount == 1,
            CustomSnapshotAgentImpl.normalInitializationCount == 0,
            restoredValue == 42,
            received.identity[String](0) == "fresh"
          )
        }
      },
      test("erased restoration supplies fresh config and activates none when restoration fails") {
        val identity = SchemaPayload.encode[Unit](())(InputRecordCodec.unit)
        ZIO.fromFuture { implicit ec =>
          ConfigHolder.clear()
          for {
            restored <- FutureInterop.fromPromise(
                          configDefn.restoreAny(
                            Array.emptyByteArray,
                            "config-snapshot-agent",
                            identity,
                            None,
                            Principal.Anonymous
                          )
                        )
            freshConfig = ConfigSnapshotAgentImpl.restoredConfig
            failed     <- FutureInterop
                        .fromPromise(
                          configDefn.restoreAny(
                            Array[Byte](1),
                            "config-snapshot-agent",
                            identity,
                            None,
                            Principal.Anonymous
                          )
                        )
                        .failed
            configStillInactive = scala.util.Try(ConfigHolder.current[RestoreConfig]).isFailure
          } yield assertTrue(
            restored.instance.isInstanceOf[ConfigSnapshotAgentImpl],
            restored.config == freshConfig,
            failed.getMessage == "expected an empty snapshot",
            configStillInactive
          )
        }
      }
    ),
    suite("Guest restoration lifecycle")(
      test("binary restoration skips normal initialization and installs principal and state") {
        val identity  = SchemaPayload.encode[String]("fresh")(InputRecordCodec.single[String]("name"))
        val principal = Principal.GolemUser(golem.Uuid(BigInt(5), BigInt(6)))
        ZIO.fromFuture { implicit ec =>
          val _ = customDefn
          Guest.resetForTesting()
          CustomSnapshotAgentImpl.normalInitializationCount = 0
          installTestHost("custom-snapshot-agent-id", "custom-snapshot-agent", identity)
          for {
            _ <- FutureInterop.fromPromise(
                   Guest.LoadSnapshot.load(
                     JsSnapshot(
                       toUint8Array(binaryEnvelope(principal, Array[Byte](0, 0, 0, 42))),
                       "application/octet-stream"
                     )
                   )
                 )
            saved  <- FutureInterop.fromPromise(Guest.SaveSnapshot.save())
            decoded = Guest
                        .decodeSnapshotPayload(fromUint8Array(saved.payload), saved.mimeType)
                        .fold(error => throw new RuntimeException(error), result => result)
            runtimeState = Guest.stateForTesting
            _            = Guest.resetForTesting()
          } yield assertTrue(
            CustomSnapshotAgentImpl.normalInitializationCount == 0,
            runtimeState == (true, Some(principal)),
            decoded._1 == principal,
            decoded._2.toSeq == Array[Byte](0, 0, 0, 42).toSeq
          )
        }
      },
      test("JSON restoration dispatches the generated state factory through Guest") {
        val identity  = SchemaPayload.encode[Unit](())(InputRecordCodec.unit)
        val principal = Principal.Agent(golem.Uuid(BigInt(7), BigInt(8)), "caller")
        ZIO.fromFuture { implicit ec =>
          val _ = autoDefn
          Guest.resetForTesting()
          installTestHost("auto-snapshot-agent-id", "auto-snapshot-agent", identity)
          for {
            _ <- FutureInterop.fromPromise(
                   Guest.LoadSnapshot.load(
                     JsSnapshot(
                       toUint8Array(jsonEnvelope(principal, """{"counter":7,"label":"loaded"}""")),
                       "application/json"
                     )
                   )
                 )
            saved  <- FutureInterop.fromPromise(Guest.SaveSnapshot.save())
            decoded = Guest
                        .decodeSnapshotPayload(fromUint8Array(saved.payload), saved.mimeType)
                        .fold(error => throw new RuntimeException(error), result => result)
            state        = new String(decoded._2, "UTF-8")
            runtimeState = Guest.stateForTesting
            _            = Guest.resetForTesting()
          } yield assertTrue(
            runtimeState == (true, Some(principal)),
            decoded._1 == principal,
            state.contains("\"counter\":7"),
            state.contains("\"label\":\"loaded\"")
          )
        }
      },
      test("failed restoration installs no runtime state and a later restoration can succeed") {
        val identity  = SchemaPayload.encode[Unit](())(InputRecordCodec.unit)
        val principal = Principal.GolemUser(golem.Uuid(BigInt(9), BigInt(10)))
        ZIO.fromFuture { implicit ec =>
          val _ = configDefn
          Guest.resetForTesting()
          installTestHost("config-snapshot-agent-id", "config-snapshot-agent", identity)
          for {
            _ <- FutureInterop
                   .fromPromise(
                     Guest.LoadSnapshot.load(
                       JsSnapshot(
                         toUint8Array(binaryEnvelope(principal, Array[Byte](1))),
                         "application/octet-stream"
                       )
                     )
                   )
                   .failed
            failedState        = Guest.stateForTesting
            configAfterFailure = scala.util.Try(ConfigHolder.current[RestoreConfig]).isSuccess
            savedAfterFailure <- FutureInterop.fromPromise(Guest.SaveSnapshot.save())
            _                 <- FutureInterop.fromPromise(
                   Guest.LoadSnapshot.load(
                     JsSnapshot(
                       toUint8Array(binaryEnvelope(principal, Array.emptyByteArray)),
                       "application/octet-stream"
                     )
                   )
                 )
            successfulState    = Guest.stateForTesting
            configAfterSuccess = scala.util.Try(ConfigHolder.current[RestoreConfig]).isSuccess
            _                  = Guest.resetForTesting()
          } yield assertTrue(
            failedState == (false, None),
            !configAfterFailure,
            savedAfterFailure.payload.length == 0,
            successfulState == (true, Some(principal)),
            configAfterSuccess
          )
        }
      }
    ) @@ TestAspect.sequential,
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
            _        <- instance.increment()
            _        <- instance.increment()
            payload  <- autoDefn.snapshotHandlers.get.save(instance)
            restored <- autoDefn.snapshotHandlers.get.load(payload.bytes, restoreContext)
            v        <- restored.increment() // counter was 2, now should be 3
          } yield assertTrue(v == 3)
        }
      }
    )
  )
}
