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

package golem.codegen.ir

/**
 * Surface IR model for agent-level metadata.
 *
 * This is the interchange format between the macro-export prepass and the RPC
 * code generator. It captures trait-level surface information sufficient for
 * generating RPC helper objects.
 */
object AgentSurfaceIR {

  final case class Module(
    agents: List[AgentSurface]
  )

  final case class MethodSurface(
    name: String,
    params: List[ParamSurface],
    returnTypeExpr: String,
    principalParams: List[Boolean]
  )

  final case class ConfigFieldSurface(
    path: List[String],
    typeExpr: String
  )

  final case class AgentSurface(
    traitFqn: String,
    packageName: String,
    simpleName: String,
    typeName: String,
    constructor: ConstructorSurface,
    metadata: AgentMetadataSurface,
    methods: List[MethodSurface],
    configFields: List[ConfigFieldSurface] = Nil
  )

  final case class ConstructorSurface(
    params: List[ParamSurface]
  )

  final case class ParamSurface(
    name: String,
    typeExpr: String
  )

  final case class AgentMetadataSurface(
    description: Option[String],
    mode: String,
    snapshotting: String
  )
}
