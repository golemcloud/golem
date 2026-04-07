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

package golem.runtime.autowire

import golem.Principal
import golem.config.{ConfigHolder, ConfigLoader}
import golem.data.GolemSchema
import golem.data.StructuredSchema
import golem.runtime.{AgentImplementationType, AsyncImplementationMethod, ConstructorMetadata, SyncImplementationMethod}

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
      implType.idSchema.schema match {
        case StructuredSchema.Tuple(elements) if elements.isEmpty =>
          AgentConstructor.noArgs[Trait](
            description = implType.metadata.description.getOrElse(typeName),
            prompt = None
          )((principal: Principal) => effectiveBuild(().asInstanceOf[Ctor], principal))
        case _ =>
          // Use the metadata constructor schema (from class Id) for param names,
          // but delegate to the GolemSchema for encoding/decoding.
          // The metadata schema has named params (e.g. "region", "catalog") while
          // the GolemSchema may use tuple names (e.g. "_1", "_2"). We need to
          // translate between them during decoding.
          val baseSchema: GolemSchema[Ctor]          = implType.idSchema
          val metadataSchema                         = implType.metadata.constructor
          implicit val ctorSchema: GolemSchema[Ctor] = new GolemSchema[Ctor] {
            override val schema: StructuredSchema                  = metadataSchema
            override def encode(value: Ctor)                       = baseSchema.encode(value)
            override def decode(value: golem.data.StructuredValue) = {
              // Translate named params from metadata schema names back to base schema names
              val translated = (value, baseSchema.schema, metadataSchema) match {
                case (
                      golem.data.StructuredValue.Tuple(metaElems),
                      StructuredSchema.Tuple(baseFields),
                      StructuredSchema.Tuple(metaFields)
                    ) if metaElems.length == baseFields.length && baseFields.length == metaFields.length =>
                  val renamed = metaElems.zip(baseFields).map { case (elem, baseField) =>
                    golem.data.NamedElementValue(baseField.name, elem.value)
                  }
                  golem.data.StructuredValue.Tuple(renamed)
                case _ => value
              }
              baseSchema.decode(translated)
            }
            override def elementSchema                                 = baseSchema.elementSchema
            override def encodeElement(value: Ctor)                    = baseSchema.encodeElement(value)
            override def decodeElement(value: golem.data.ElementValue) = baseSchema.decodeElement(value)
          }
          AgentConstructor.sync[Ctor, Trait](
            ConstructorMetadata(
              name = None,
              description = implType.metadata.description.getOrElse(typeName),
              promptHint = None
            )
          )(effectiveBuild)
      }

    val bindings = implType.methods.map {
      case sync: SyncImplementationMethod[Trait @unchecked, in, out] =>
        buildSyncBinding[Trait, in, out](sync)
      case async: AsyncImplementationMethod[Trait @unchecked, in, out] =>
        buildAsyncBinding[Trait, in, out](async)
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

  private def buildSyncBinding[Trait, In, Out](
    method: SyncImplementationMethod[Trait, In, Out]
  ): MethodBinding[Trait] = {
    implicit val inSchema: GolemSchema[In]   = method.inputSchema
    implicit val outSchema: GolemSchema[Out] = method.outputSchema

    MethodBinding.sync[Trait, In, Out](method.metadata) { (instance, input, principal) =>
      method.handler(instance, input, principal)
    }
  }

  private def buildAsyncBinding[Trait, In, Out](
    method: AsyncImplementationMethod[Trait, In, Out]
  ): MethodBinding[Trait] = {
    implicit val inSchema: GolemSchema[In]   = method.inputSchema
    implicit val outSchema: GolemSchema[Out] = method.outputSchema

    MethodBinding.async[Trait, In, Out](method.metadata) { (instance, input, principal) =>
      method.handler(instance, input, principal)
    }
  }
}
