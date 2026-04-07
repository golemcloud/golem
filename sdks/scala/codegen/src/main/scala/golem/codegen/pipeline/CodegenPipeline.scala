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

package golem.codegen.pipeline

import golem.codegen.autoregister.AutoRegisterCodegen
import golem.codegen.discovery.SourceDiscovery
import golem.codegen.ir.AgentSurfaceIR
import golem.codegen.rpc.RpcCodegen

/**
 * Shared codegen pipeline consumed by both sbt and mill plugins.
 *
 * Encapsulates the flow: discovery → IR conversion → auto-register + RPC
 * generation. Build plugins only need to handle file I/O, logging, and
 * formatting.
 */
object CodegenPipeline {

  final case class GeneratedFile(relativePath: String, content: String)

  final case class PipelineResult(
    autoRegister: Option[AutoRegisterResult],
    rpc: RpcResult
  )

  final case class AutoRegisterResult(
    files: Seq[GeneratedFile],
    warnings: Seq[String],
    generatedPackage: String,
    implCount: Int,
    packageCount: Int
  )

  final case class RpcResult(
    files: Seq[GeneratedFile],
    warnings: Seq[String]
  )

  /**
   * Run the full codegen pipeline from discovery results.
   *
   * @param discovered
   *   the discovery results from scanning source files
   * @param basePackageOpt
   *   base package for auto-register generation (None to skip)
   * @param rpcEnabled
   *   whether to generate RPC client objects
   * @return
   *   pipeline results with generated files and warnings
   */
  def run(
    discovered: SourceDiscovery.Result,
    basePackageOpt: Option[String],
    rpcEnabled: Boolean
  ): PipelineResult = {
    val autoReg = basePackageOpt.map { basePackage =>
      val result = AutoRegisterCodegen.generateFromDiscovery(basePackage, discovered)
      AutoRegisterResult(
        files = result.files.map(f => GeneratedFile(f.relativePath, f.content)),
        warnings = result.warnings.map(_.message),
        generatedPackage = result.generatedPackage,
        implCount = result.implCount,
        packageCount = result.packageCount
      )
    }

    val rpc =
      if (!rpcEnabled) RpcResult(Nil, Nil)
      else {
        val agents = discoveredToIR(discovered)
        val result = RpcCodegen.generate(agents, discovered.objects)
        RpcResult(
          files = result.files.map(f => GeneratedFile(f.relativePath, f.content)),
          warnings = result.warnings.map(_.message)
        )
      }

    PipelineResult(autoRegister = autoReg, rpc = rpc)
  }

  /** Convert discovered traits to IR agent surfaces. */
  private def discoveredToIR(discovered: SourceDiscovery.Result): List[AgentSurfaceIR.AgentSurface] =
    discovered.traits.map { t =>
      val fqn = if (t.pkg.isEmpty) t.name else s"${t.pkg}.${t.name}"
      AgentSurfaceIR.AgentSurface(
        traitFqn = fqn,
        packageName = t.pkg,
        simpleName = t.name,
        typeName = t.typeName.getOrElse(t.name),
        constructor = AgentSurfaceIR.ConstructorSurface(
          t.constructorParams.map(p => AgentSurfaceIR.ParamSurface(p.name, p.typeExpr))
        ),
        metadata = AgentSurfaceIR.AgentMetadataSurface(
          description = t.descriptionValue,
          mode = t.mode.getOrElse("durable"),
          snapshotting = "disabled"
        ),
        methods = t.methods.map(m =>
          AgentSurfaceIR.MethodSurface(
            name = m.name,
            params = m.params.map(p => AgentSurfaceIR.ParamSurface(p.name, p.typeExpr)),
            returnTypeExpr = m.returnTypeExpr,
            principalParams = m.principalParams
          )
        ),
        configFields =
          t.configFields.map(cf => AgentSurfaceIR.ConfigFieldSurface(path = cf.path, typeExpr = cf.typeExpr))
      )
    }.toList
}
