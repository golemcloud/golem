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

import scala.compiletime.testing.{Error, typeCheckErrors}
import zio.test._

object SnapshotMacroCompileErrorsSpec extends ZIOSpecDefault {
  def spec = suite("SnapshotMacroCompileErrorsSpec")(
    test("instance save without companion load reports the required pair") {
      val errors: List[Error] = typeCheckErrors("""
        import golem.*
        import golem.runtime.annotations.*
        import golem.runtime.autowire.AgentImplementation
        import scala.concurrent.Future

        @agentDefinition(snapshotting = "enabled")
        trait MissingLoadAgent extends BaseAgent { class Id() }

        @agentImplementation()
        final class MissingLoadImpl() extends MissingLoadAgent {
          def saveSnapshot(): Future[Array[Byte]] = Future.successful(Array.emptyByteArray)
        }

        AgentImplementation.registerClass[MissingLoadAgent, MissingLoadImpl]
      """)
      assertTrue(errors.exists(_.message.contains("instance saveSnapshot")))
    },
    test("companion load without instance save reports the required pair") {
      val errors: List[Error] = typeCheckErrors("""
        import golem.*
        import golem.runtime.annotations.*
        import golem.runtime.autowire.AgentImplementation
        import scala.concurrent.Future

        @agentDefinition(snapshotting = "enabled")
        trait MissingSaveAgent extends BaseAgent { class Id() }

        @agentImplementation()
        final class MissingSaveImpl() extends MissingSaveAgent
        object MissingSaveImpl {
          def loadSnapshot(bytes: Array[Byte], context: SnapshotRestoreContext): Future[MissingSaveImpl] =
            Future.successful(new MissingSaveImpl())
        }

        AgentImplementation.registerClass[MissingSaveAgent, MissingSaveImpl]
      """)
      assertTrue(errors.exists(_.message.contains("instance saveSnapshot")))
    },
    test("wrong custom save result reports the exact contract") {
      val errors: List[Error] = typeCheckErrors("""
        import golem.*
        import golem.runtime.annotations.*
        import golem.runtime.autowire.AgentImplementation
        import scala.concurrent.Future

        @agentDefinition(snapshotting = "enabled")
        trait MalformedHooksAgent extends BaseAgent { class Id() }

        @agentImplementation()
        final class MalformedHooksImpl() extends MalformedHooksAgent {
          def saveSnapshot(): Future[String] = Future.successful("wrong")
        }
        object MalformedHooksImpl {
          def loadSnapshot(bytes: Array[Byte], context: SnapshotRestoreContext): Future[MalformedHooksImpl] =
            Future.successful(new MalformedHooksImpl())
        }

        AgentImplementation.registerClass[MalformedHooksAgent, MalformedHooksImpl]
      """)
      assertTrue(errors.exists(_.message.contains("Future[Array[Byte]]")))
    },
    test("curried custom loader reports the exact contract") {
      val errors: List[Error] = typeCheckErrors("""
        import golem.*
        import golem.runtime.annotations.*
        import golem.runtime.autowire.AgentImplementation
        import scala.concurrent.Future

        @agentDefinition(snapshotting = "enabled")
        trait CurriedLoadAgent extends BaseAgent { class Id() }

        @agentImplementation()
        final class CurriedLoadImpl() extends CurriedLoadAgent {
          def saveSnapshot(): Future[Array[Byte]] = Future.successful(Array.emptyByteArray)
        }
        object CurriedLoadImpl {
          def loadSnapshot(bytes: Array[Byte])(context: SnapshotRestoreContext): Future[CurriedLoadImpl] =
            Future.successful(new CurriedLoadImpl())
        }

        AgentImplementation.registerClass[CurriedLoadAgent, CurriedLoadImpl]
      """)
      assertTrue(errors.exists(_.message.contains("additional parameter lists")))
    },
    test("generic custom hooks report the exact contract") {
      val errors: List[Error] = typeCheckErrors("""
        import golem.*
        import golem.runtime.annotations.*
        import golem.runtime.autowire.AgentImplementation
        import scala.concurrent.Future

        @agentDefinition(snapshotting = "enabled")
        trait GenericHooksAgent extends BaseAgent { class Id() }

        @agentImplementation()
        final class GenericHooksImpl() extends GenericHooksAgent {
          def saveSnapshot[A](): Future[Array[Byte]] = Future.successful(Array.emptyByteArray)
        }
        object GenericHooksImpl {
          def loadSnapshot[A](bytes: Array[Byte], context: SnapshotRestoreContext): Future[GenericHooksImpl] =
            Future.successful(new GenericHooksImpl())
        }

        AgentImplementation.registerClass[GenericHooksAgent, GenericHooksImpl]
      """)
      assertTrue(errors.exists(_.message.contains("no type parameters")))
    },
    test("private custom hooks report the public contract") {
      val errors: List[Error] = typeCheckErrors("""
        import golem.*
        import golem.runtime.annotations.*
        import golem.runtime.autowire.AgentImplementation
        import scala.concurrent.Future

        @agentDefinition(snapshotting = "enabled")
        trait PrivateHooksAgent extends BaseAgent { class Id() }

        @agentImplementation()
        final class PrivateHooksImpl() extends PrivateHooksAgent {
          private def saveSnapshot(): Future[Array[Byte]] = Future.successful(Array.emptyByteArray)
        }
        object PrivateHooksImpl {
          private def loadSnapshot(
            bytes: Array[Byte],
            context: SnapshotRestoreContext
          ): Future[PrivateHooksImpl] = Future.successful(new PrivateHooksImpl())
        }

        AgentImplementation.registerClass[PrivateHooksAgent, PrivateHooksImpl]
      """)
      assertTrue(errors.exists(_.message.contains("must declare exactly instance saveSnapshot")))
    },
    test("Snapshotted state without companion load reports its signature") {
      val errors: List[Error] = typeCheckErrors("""
        import golem.*
        import golem.runtime.annotations.*
        import golem.runtime.autowire.AgentImplementation
        import zio.blocks.schema.Schema

        final case class MissingLoadState(value: Int) derives Schema

        @agentDefinition(snapshotting = "enabled")
        trait MissingStateLoadAgent extends BaseAgent { class Id() }

        @agentImplementation()
        final class MissingStateLoadImpl() extends MissingStateLoadAgent with Snapshotted[MissingLoadState] {
          var state: MissingLoadState = MissingLoadState(0)
        }

        AgentImplementation.registerClass[MissingStateLoadAgent, MissingStateLoadImpl]
      """)
      assertTrue(errors.exists(_.message.contains("must declare exactly public companion loadSnapshot(state:")))
    },
    test("private Snapshotted loader reports the public companion contract") {
      val errors: List[Error] = typeCheckErrors("""
        import golem.*
        import golem.runtime.annotations.*
        import golem.runtime.autowire.AgentImplementation
        import zio.blocks.schema.Schema
        import scala.concurrent.Future

        final case class PrivateLoadState(value: Int) derives Schema

        @agentDefinition(snapshotting = "enabled")
        trait PrivateLoadAgent extends BaseAgent { class Id() }

        @agentImplementation()
        final class PrivateLoadImpl() extends PrivateLoadAgent with Snapshotted[PrivateLoadState] {
          var state: PrivateLoadState = PrivateLoadState(0)
        }
        object PrivateLoadImpl {
          private def loadSnapshot(
            state: PrivateLoadState,
            context: SnapshotRestoreContext
          ): Future[PrivateLoadImpl] = Future.successful(new PrivateLoadImpl())
        }

        AgentImplementation.registerClass[PrivateLoadAgent, PrivateLoadImpl]
      """)
      assertTrue(errors.exists(_.message.contains("exactly public companion loadSnapshot")))
    },
    test("curried Snapshotted loader reports the exact companion contract") {
      val errors: List[Error] = typeCheckErrors("""
        import golem.*
        import golem.runtime.annotations.*
        import golem.runtime.autowire.AgentImplementation
        import zio.blocks.schema.Schema
        import scala.concurrent.Future

        final case class CurriedLoadState(value: Int) derives Schema

        @agentDefinition(snapshotting = "enabled")
        trait CurriedStateLoadAgent extends BaseAgent { class Id() }

        @agentImplementation()
        final class CurriedStateLoadImpl() extends CurriedStateLoadAgent with Snapshotted[CurriedLoadState] {
          var state: CurriedLoadState = CurriedLoadState(0)
        }
        object CurriedStateLoadImpl {
          def loadSnapshot(
            state: CurriedLoadState
          )(context: SnapshotRestoreContext): Future[CurriedStateLoadImpl] =
            Future.successful(new CurriedStateLoadImpl())
        }

        AgentImplementation.registerClass[CurriedStateLoadAgent, CurriedStateLoadImpl]
      """)
      assertTrue(errors.exists(_.message.contains("additional parameter lists")))
    },
    test("generic Snapshotted loader reports the exact companion contract") {
      val errors: List[Error] = typeCheckErrors("""
        import golem.*
        import golem.runtime.annotations.*
        import golem.runtime.autowire.AgentImplementation
        import zio.blocks.schema.Schema
        import scala.concurrent.Future

        final case class GenericLoadState(value: Int) derives Schema

        @agentDefinition(snapshotting = "enabled")
        trait GenericStateLoadAgent extends BaseAgent { class Id() }

        @agentImplementation()
        final class GenericStateLoadImpl() extends GenericStateLoadAgent with Snapshotted[GenericLoadState] {
          var state: GenericLoadState = GenericLoadState(0)
        }
        object GenericStateLoadImpl {
          def loadSnapshot[A](
            state: GenericLoadState,
            context: SnapshotRestoreContext
          ): Future[GenericStateLoadImpl] = Future.successful(new GenericStateLoadImpl())
        }

        AgentImplementation.registerClass[GenericStateLoadAgent, GenericStateLoadImpl]
      """)
      assertTrue(errors.exists(_.message.contains("no type parameters")))
    }
  )
}
