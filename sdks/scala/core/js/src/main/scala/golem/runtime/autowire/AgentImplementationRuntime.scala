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
import golem.runtime.{AgentImplementationType, AsyncImplementationMethod, SyncImplementationMethod}

private[autowire] object AgentImplementationRuntime {
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
      metadata = implType.metadata,
      constructor = constructor,
      bindings = bindings,
      mode = mode,
      snapshotHandlers = implType.snapshotHandlers
    )

    AgentRegistry.register(definition)
    definition
  }
}
