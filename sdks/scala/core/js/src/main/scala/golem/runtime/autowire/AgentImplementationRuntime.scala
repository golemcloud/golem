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

package golem.runtime.autowire

import golem.Principal
import golem.config.{ConfigHolder, ConfigLoader}
import golem.host.js.schema.JsAgentError
import golem.runtime.{
  AgentImplementationType,
  AsyncImplementationMethod,
  SnapshotRestoreContext,
  SyncImplementationMethod,
  WireAgentImplementationType,
  WireAgentInputError,
  WireAgentMetadata
}
import golem.FutureInterop

import scala.concurrent.Future
import scala.scalajs.js
import scala.util.control.NonFatal

private[autowire] object AgentImplementationRuntime {
  private def identityFields(ctor: Any): Vector[Any] = ctor match {
    case ()                => Vector.empty
    case values: Vector[?] => values.asInstanceOf[Vector[Any]]
    case value             => Vector(value)
  }

  def registerWire[Trait, Ctor](
    typeName: String,
    mode: AgentMode,
    implType: WireAgentImplementationType[Trait, Ctor]
  ): AgentDefinition[Trait] = {
    val effectiveBuild: (Ctor, Principal) => Trait = implType.configBuilder match {
      case Some(builder) if !implType.configInjectedViaConstructor =>
        (ctor: Ctor, principal: Principal) => {
          ConfigHolder.set(ConfigLoader.loadConfig(builder))
          implType.buildInstance(ctor, principal)
        }
      case _ => implType.buildInstance
    }
    val definition = new AgentDefinition[Trait](
      typeName = typeName,
      descriptor = implType.metadata,
      constructor = AgentConstructor.wire(implType.metadata, implType.ctorCodec)(effectiveBuild),
      bindings = implType.methods.map(MethodBinding.wire(implType.metadata, _)),
      mode = mode,
      snapshotHandlers = implType.snapshotHandlers,
      restoreInstance = implType.snapshotHandlers.map { handlers => (bytes, agentId, constructorInput, principal) =>
        FutureInterop.toPromise(SchemaPayload.withWireInput(constructorInput) { value =>
          val ctor =
            try implType.ctorCodec.decode(value)
            catch { case NonFatal(error) => throw WireAgentInputError(String.valueOf(error.getMessage)) }
          val freshConfig = implType.configBuilder.map(builder => ConfigLoader.createLazyConfig(builder))
          val context     = SnapshotRestoreContext(identityFields(ctor), agentId, principal, freshConfig)
          handlers
            .load(bytes, context)
            .map(instance => RestoredInstance(instance, freshConfig))(
              scala.concurrent.ExecutionContext.parasitic
            )
        })
      }
    )
    AgentRegistry.register(definition)
    definition
  }

  def register[Trait, Ctor](
    typeName: String,
    mode: AgentMode,
    implType: AgentImplementationType[Trait, Ctor]
  ): AgentDefinition[Trait] = {
    val effectiveBuild: (Ctor, Principal) => Trait = implType.configBuilder match {
      case Some(builder) if !implType.configInjectedViaConstructor =>
        (ctor: Ctor, principal: Principal) => {
          val config = ConfigLoader.loadConfig(builder)
          ConfigHolder.set(config)
          implType.buildInstance(ctor, principal)
        }
      case _ => implType.buildInstance
    }

    val constructor =
      AgentConstructor.sync[Ctor, Trait](implType.metadata.constructor, implType.ctorCodec)(effectiveBuild)

    val bindings = implType.methods.map {
      case sync: SyncImplementationMethod[Trait @unchecked, in, out] =>
        MethodBinding.sync[Trait, in, out](sync.metadata, sync.inputCodec, sync.outputCodec)(sync.handler)
      case async: AsyncImplementationMethod[Trait @unchecked, in, out] =>
        MethodBinding.async[Trait, in, out](async.metadata, async.inputCodec, async.outputCodec)(async.handler)
    }

    val definition = new AgentDefinition[Trait](
      typeName = typeName,
      descriptor = WireAgentMetadata.fromModel(implType.metadata),
      constructor = constructor,
      bindings = bindings,
      mode = mode,
      snapshotHandlers = implType.snapshotHandlers,
      restoreInstance = implType.snapshotHandlers.map { handlers => (bytes, agentId, constructorInput, principal) =>
        FutureInterop.toPromise(
          SchemaPayload.withDecodedInput[Ctor, RestoredInstance[Trait]](constructorInput) {
            case Left(err)   => Future.failed(js.JavaScriptException(JsAgentError.invalidInput(err.toString)))
            case Right(ctor) =>
              val freshConfig = implType.configBuilder.map(builder => ConfigLoader.createLazyConfig(builder))
              val context     = SnapshotRestoreContext(identityFields(ctor), agentId, principal, freshConfig)
              handlers
                .load(bytes, context)
                .map(instance => RestoredInstance(instance, freshConfig))(
                  scala.concurrent.ExecutionContext.parasitic
                )
          }(implType.ctorCodec)
        )
      }
    )

    AgentRegistry.register(definition)
    definition
  }
}
