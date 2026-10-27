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
    configFields: List[ConfigFieldSurface] = Nil,
    sourceScopes: List[SourceScopeSurface] = Nil
  )

  /**
   * One level of the package clauses that enclose the agent trait in its source
   * file, with the import statements of that level which precede the trait, as
   * written. An empty `packageRef` is the level outside every package clause.
   * The non-empty references, joined with dots, form the agent's package name.
   */
  final case class SourceScopeSurface(
    packageRef: String,
    imports: List[String]
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
    snapshotting: String,
    kind: String = "regular"
  )
}
