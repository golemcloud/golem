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

package golem.codegen.autoregister

import golem.codegen.discovery.SourceDiscovery

import java.security.MessageDigest

import scala.meta._
import scala.meta.dialects.Scala213
import scala.meta.parsers._

/**
 * Pure, build-tool-agnostic code generator for Golem agent/tool
 * auto-registration.
 *
 * Scans Scala source text for `@agentImplementation` and `@toolImplementation`
 * annotated classes and produces Scala source files that register them via
 * `AgentImplementation.registerClass` and `ToolImplementation.registerClass`.
 *
 * All generated code is constructed as scalameta AST nodes and pretty-printed
 * via `.syntax`, following the project's code-generation conventions.
 */
object AutoRegisterCodegen {

  final case class SourceInput(path: String, content: String)

  final case class Warning(path: Option[String], message: String)

  final case class GeneratedFile(relativePath: String, content: String)

  final case class Result(
    generatedPackage: String,
    files: Seq[GeneratedFile],
    warnings: Seq[Warning],
    implCount: Int,
    packageCount: Int
  )

  /** Computes the generated package name for a given base package. */
  def generatedPackage(basePackage: String): String =
    s"golem.runtime.__generated.autoregister.${autoRegisterSuffix(basePackage)}"

  /**
   * Generates auto-registration source files for all `@agentImplementation` and
   * `@toolImplementation` classes found in the given sources.
   *
   * @param basePackage
   *   the user's base package (e.g. `"example"`)
   * @param sources
   *   Scala source inputs to scan
   * @return
   *   a [[Result]] with generated files (relative paths) and any warnings
   */
  def generate(basePackage: String, sources: Seq[SourceInput]): Result = {
    val discoveryInputs = sources.map(s => SourceDiscovery.SourceInput(s.path, s.content))
    val discovered      = SourceDiscovery.discover(discoveryInputs)
    generateFromDiscovery(basePackage, discovered)
  }

  /**
   * Generates auto-registration source files from pre-computed discovery
   * output.
   *
   * This is the preferred entry point when discovery has already been performed
   * (e.g. when shared between auto-register and RPC codegen).
   */
  def generateFromDiscovery(basePackage: String, discovered: SourceDiscovery.Result): Result = {
    val genBasePkg = generatedPackage(basePackage)

    val warnings: Seq[Warning] = discovered.warnings.map(w => Warning(w.path, w.message))

    val agentImpls: List[AgentImpl] =
      discovered.implementations
        .map(di => resolveAgentImpl(di, discovered.traits))
        .toList
        .distinct
        .sortBy(ai => (ai.pkg, ai.traitType, ai.implClass))

    val toolImpls: List[ToolImpl] =
      discovered.toolImplementations
        .map(di => resolveToolImpl(di, discovered.tools))
        .toList
        .distinct
        .sortBy(ti => (ti.pkg, ti.traitType, ti.implClass))

    if (agentImpls.isEmpty && toolImpls.isEmpty) {
      Result(
        generatedPackage = genBasePkg,
        files = Seq.empty,
        warnings = warnings,
        implCount = 0,
        packageCount = 0
      )
    } else {
      val packages                               = (agentImpls.map(_.pkg) ++ toolImpls.map(_.pkg)).distinct.sorted
      val byPkg: Map[String, List[Registration]] =
        packages.map { pkg =>
          val registrations =
            agentImpls.filter(_.pkg == pkg).map(Registration.Agent(_)) ++
              toolImpls.filter(_.pkg == pkg).map(Registration.Tool(_))
          pkg -> registrations.sortBy {
            case Registration.Agent(ai) => ("agent", ai.traitType, ai.implClass)
            case Registration.Tool(ti)  => ("tool", ti.traitType, ti.implClass)
          }
        }.toMap

      val perPkgFiles: Seq[GeneratedFile] =
        byPkg.toSeq.sortBy(_._1).map { case (pkg, registrations) =>
          val objSuffix = sanitizeSuffix(pkg)
          val tree      = buildPerPkgSource(
            genBasePkg,
            objSuffix,
            registrations,
            surfaceFingerprint(registrations, discovered.traits, discovered.tools, discovered.sourceHashes)
          )
          GeneratedFile(
            relativePath = packagePath(genBasePkg, s"__GolemAutoRegister_$objSuffix.scala"),
            content = tree.syntax
          )
        }

      val registerCallExprs: List[Stat] =
        byPkg.keys.toSeq.sorted.toList.map { pkg =>
          val objSuffix = sanitizeSuffix(pkg)
          val objRef    = Term.Name(s"__GolemAutoRegister_$objSuffix")
          q"$objRef.register()"
        }

      val baseTree = buildRegisterAgentsSource(genBasePkg, registerCallExprs)
      val baseFile = GeneratedFile(
        relativePath = packagePath(genBasePkg, "RegisterAgents.scala"),
        content = baseTree.syntax
      )

      Result(
        generatedPackage = genBasePkg,
        files = perPkgFiles :+ baseFile,
        warnings = warnings,
        implCount = agentImpls.length + toolImpls.length,
        packageCount = byPkg.size
      )
    }
  }

  // ── AST construction ───────────────────────────────────────────────────────

  private def buildPerPkgSource(
    genBasePkg: String,
    objSuffix: String,
    registrations: List[Registration],
    surfaceFingerprint: String
  ): Source = {
    val pkgRef  = parseTermRef(genBasePkg)
    val objName = Term.Name(s"__GolemAutoRegister_$objSuffix")

    val registrationStats: List[Stat] = registrations.map {
      case Registration.Agent(ai) => buildAgentRegistrationCall(ai)
      case Registration.Tool(ti)  => buildToolRegistrationCall(ti)
    }

    val imports: List[Stat] =
      List(
        if (registrations.exists(_.isInstanceOf[Registration.Agent]))
          Some(parseMeta[Stat]("import golem.runtime.autowire.AgentImplementation"))
        else None,
        if (registrations.exists(_.isInstanceOf[Registration.Tool]))
          Some(parseMeta[Stat]("import golem.runtime.autowire.ToolImplementation"))
        else None
      ).flatten

    source"""
      package $pkgRef {
        ..$imports

        /** Generated. Do not edit. */
        private[golem] object $objName {
          private val __golemSurfaceVersion = ${Lit.String(surfaceFingerprint)}

          def register(): Unit = {
            ..$registrationStats
            ()
          }
        }
      }
    """
  }

  private def buildRegisterAgentsSource(
    genBasePkg: String,
    registerCalls: List[Stat]
  ): Source = {
    val pkgRef = parseTermRef(genBasePkg)

    val jsExportImport = parseImporter("scala.scalajs.js.annotation.JSExportTopLevel")

    val jsExportAnnot = parseMeta[Mod]("""@JSExportTopLevel("__golemRegisterAgents")""")

    source"""
      package $pkgRef {
        import ..$jsExportImport

        /** Generated. Do not edit. */
        private[golem] object RegisterAgents {
          private var registered = false

          private def registerAll(): Unit =
            if (!registered) {
              registered = true
              ..$registerCalls
              ()
            }

          def main(): Unit = registerAll()

          $jsExportAnnot val __golemRegisterAgents: Unit =
            registerAll()
        }
      }
    """
  }

  private def buildAgentRegistrationCall(ai: AgentImpl): Stat = {
    val traitTpe = parseType(fqn(ai.pkg, ai.traitType))
    val implTpe  = parseType(fqn(ai.pkg, ai.implClass))
    q"AgentImplementation.registerClass[$traitTpe, $implTpe]"
  }

  private def buildToolRegistrationCall(ti: ToolImpl): Stat = {
    val traitTpe = parseType(fqn(ti.pkg, ti.traitType))
    val implTpe  = parseType(fqn(ti.pkg, ti.implClass))
    q"ToolImplementation.registerClass[$traitTpe, $implTpe]"
  }

  // ── Type/term reference helpers ────────────────────────────────────────────

  private def parseMeta[T](code: String)(implicit parse: Parse[T]): T =
    Scala213(code).parse[T].get

  private def parseTermRef(dotted: String): Term.Ref =
    parseMeta[Term](dotted).asInstanceOf[Term.Ref]

  private def parseType(tpe: String): Type =
    parseMeta[Type](tpe)

  private def parseImporter(dotted: String): List[Importer] = {
    val importStat = parseMeta[Stat](s"import $dotted").asInstanceOf[Import]
    importStat.importers
  }

  // ── Implementation details ─────────────────────────────────────────────────

  private final case class AgentImpl(pkg: String, implClass: String, traitType: String, ctorTypes: List[String])

  private final case class ToolImpl(pkg: String, implClass: String, traitType: String)

  private sealed trait Registration extends Product with Serializable
  private object Registration {
    final case class Agent(value: AgentImpl) extends Registration
    final case class Tool(value: ToolImpl)   extends Registration
  }

  private trait DiscoveredSurface {
    def pkg: String
    def name: String
  }

  private final case class AgentSurface(value: SourceDiscovery.AgentTrait) extends DiscoveredSurface {
    def pkg: String  = value.pkg
    def name: String = value.name
  }

  private final case class ToolSurface(value: SourceDiscovery.ToolTrait) extends DiscoveredSurface {
    def pkg: String  = value.pkg
    def name: String = value.name
  }

  private def normalizeTypeRef(tpe: String): String =
    tpe.stripPrefix("_root_.")

  private def resolveParentTrait[S <: DiscoveredSurface](
    implPkg: String,
    parentTypes: List[String],
    imports: Map[String, String],
    wildcardImports: List[SourceDiscovery.WildcardImport],
    surfaces: Seq[S]
  ): Option[String] = {
    val byFqn  = surfaces.map(s => s"${s.pkg}.${s.name}" -> s).toMap
    val byName = surfaces.groupBy(_.name)

    def enclosingPackages: List[String] = {
      val parts = implPkg.split('.').toList.filter(_.nonEmpty)
      parts.indices.reverse.map(i => parts.take(i + 1).mkString(".")).toList
    }

    def importedCandidates(ref: String): List[String] = {
      val rooted     = ref.startsWith("_root_.")
      val normalized = normalizeTypeRef(ref)
      val relative   =
        if (!rooted && normalized.contains(".")) enclosingPackages.map(prefix => s"$prefix.$normalized")
        else Nil
      if (rooted) List(normalized)
      else (relative :+ normalized).distinct
    }

    def resolveImportedRef(ref: String): Option[String] =
      importedCandidates(ref).iterator
        .flatMap(candidate => byFqn.get(candidate))
        .map(s => s"${s.pkg}.${s.name}")
        .toSeq
        .headOption

    def expandImportedQualifier(tpe: String): String = {
      val dot = tpe.indexOf('.')
      if (dot < 0) tpe
      else {
        val qualifier = tpe.substring(0, dot)
        val rest      = tpe.substring(dot + 1)
        imports.get(qualifier).map(imported => s"${normalizeTypeRef(imported)}.$rest").getOrElse(tpe)
      }
    }

    def resolve(parent: String, allowGlobalSimpleNameFallback: Boolean): Option[String] = {
      val normalized = normalizeTypeRef(expandImportedQualifier(parent))
      resolveImportedRef(normalized).orElse {
        val samePackage = byName.get(normalized).flatMap(_.find(_.pkg == implPkg)).map(s => s"${s.pkg}.${s.name}")
        samePackage.orElse {
          imports
            .get(normalized)
            .flatMap(resolveImportedRef)
            .orElse(imports.get(normalized).map(normalizeTypeRef))
            .orElse {
              val wildcardMatches = wildcardImports
                .filterNot(_.excludes.contains(normalized))
                .flatMap(wildcard => importedCandidates(s"${wildcard.pkg}.$normalized"))
                .flatMap(candidate => byFqn.get(candidate).map(surface => s"${surface.pkg}.${surface.name}"))
                .distinct
              wildcardMatches match {
                case single :: Nil                      => Some(single)
                case _ if allowGlobalSimpleNameFallback =>
                  byName.get(normalized).map(_.toList) match {
                    case Some(single :: Nil) => Some(s"${single.pkg}.${single.name}")
                    case _                   => None
                  }
                case _ => None
              }
            }
        }
      }
    }

    parentTypes.iterator
      .flatMap(parent => resolve(parent, allowGlobalSimpleNameFallback = false))
      .toSeq
      .headOption
      .orElse(
        parentTypes.iterator
          .flatMap(parent => resolve(parent, allowGlobalSimpleNameFallback = true))
          .toSeq
          .headOption
      )
  }

  private def resolveAgentImpl(
    impl: SourceDiscovery.AgentImpl,
    traits: Seq[SourceDiscovery.AgentTrait]
  ): AgentImpl = {
    val resolvedTrait = resolveParentTrait(
      impl.pkg,
      impl.parentTypes,
      impl.imports,
      impl.wildcardImports,
      traits.map(AgentSurface.apply)
    ).getOrElse(normalizeTypeRef(impl.traitType))

    AgentImpl(impl.pkg, impl.implClass, resolvedTrait, impl.ctorTypes)
  }

  private def resolveToolImpl(
    impl: SourceDiscovery.ToolImpl,
    tools: Seq[SourceDiscovery.ToolTrait]
  ): ToolImpl = {
    val resolvedTrait = resolveParentTrait(
      impl.pkg,
      impl.parentTypes,
      impl.imports,
      impl.wildcardImports,
      tools.map(ToolSurface.apply)
    ).getOrElse(normalizeTypeRef(impl.traitType))

    ToolImpl(impl.pkg, impl.implClass, resolvedTrait)
  }

  private def surfaceFingerprint(
    registrations: List[Registration],
    traits: Seq[SourceDiscovery.AgentTrait],
    tools: Seq[SourceDiscovery.ToolTrait],
    sourceHashes: Seq[(String, String)]
  ): String = {
    val traitsByFqn  = traits.map(t => s"${t.pkg}.${t.name}" -> t).toMap
    val traitsByName = traits.map(t => (t.pkg, t.name) -> t).toMap
    val toolsByFqn   = tools.map(t => s"${t.pkg}.${t.name}" -> t).toMap
    val toolsByName  = tools.map(t => (t.pkg, t.name) -> t).toMap

    val registeredSurface = registrations.map {
      case Registration.Agent(impl) =>
        val resolvedTrait =
          traitsByName.get((impl.pkg, impl.traitType)).orElse(traitsByFqn.get(normalizeTypeRef(impl.traitType)))

        val traitSurface = resolvedTrait match {
          case Some(agentTrait) =>
            val constructor =
              agentTrait.constructorParams.map(param => s"${param.name}:${param.typeExpr}").mkString(",")
            val methods = agentTrait.methods.map { method =>
              val params = method.params.map(param => s"${param.name}:${param.typeExpr}").mkString(",")
              s"${method.name}($params)=>${method.returnTypeExpr}[${method.principalParams.mkString(",")}]"
            }.mkString(";")

            s"trait=${agentTrait.pkg}.${agentTrait.name}|typeName=${agentTrait.typeName.getOrElse(agentTrait.name)}|ctor=$constructor|methods=$methods|mode=${agentTrait.mode.getOrElse("durable")}|desc=${agentTrait.descriptionValue.getOrElse("")}"
          case None =>
            s"trait=${impl.traitType}"
        }

        s"impl=${impl.pkg}.${impl.implClass}|ctorTypes=${impl.ctorTypes.mkString(",")}|$traitSurface"
      case Registration.Tool(impl) =>
        val resolvedTool =
          toolsByName.get((impl.pkg, impl.traitType)).orElse(toolsByFqn.get(normalizeTypeRef(impl.traitType)))

        val toolSurface = resolvedTool match {
          case Some(tool) =>
            val methods = tool.methods.map { method =>
              val params = method.params.map(param => s"${param.name}:${param.typeExpr}").mkString(",")
              val args   = method.args
                .map(arg =>
                  s"${arg.name}:${arg.scope.getOrElse("")}:${arg.kind.getOrElse("")}:${arg.aliases.mkString(",")}:${arg.syntax}"
                )
                .mkString(";")
              val results     = method.resultAnnotations.mkString(";")
              val constraints = method.constraintAnnotations.mkString(";")
              val annotations = method.commandAnnotations.mkString(";")
              s"${method.name}($params)=>${method.returnTypeExpr}|cmd=${method.commandName.getOrElse("")}|aliases=${method.commandAliases.mkString(",")}|args=$args|result=$results|constraints=$constraints|annotations=$annotations"
            }.mkString(";")
            s"tool=${tool.pkg}.${tool.name}|toolName=${tool.toolName.getOrElse(tool.name)}|version=${tool.version.getOrElse("")}|source=${tool.sourceHash}|methods=$methods"
          case None =>
            s"tool=${impl.traitType}"
        }

        s"toolImpl=${impl.pkg}.${impl.implClass}|$toolSurface"
    }.mkString("\n")

    val sourceSurface =
      if (registrations.exists(_.isInstanceOf[Registration.Tool]))
        sourceHashes.map { case (path, hash) => s"source=$path:$hash" }.mkString("\n")
      else ""
    val surface = s"$registeredSurface\n$sourceSurface"

    sha256Hex(surface)
  }

  private def sha256Hex(value: String): String = {
    val digest = MessageDigest.getInstance("SHA-256")
    digest.digest(value.getBytes("UTF-8")).map(b => f"$b%02x").mkString
  }

  private def autoRegisterSuffix(basePackage: String): String =
    basePackage
      .replaceAll("[^a-zA-Z0-9_]", "_")
      .stripPrefix("_")
      .stripSuffix("_") match {
      case ""  => "app"
      case out => out
    }

  private def sanitizeSuffix(pkg: String): String =
    pkg.replaceAll("[^a-zA-Z0-9_]", "_")

  private def packagePath(pkg: String, fileName: String): String =
    pkg.replace('.', '/') + "/" + fileName

  private val scalaBuiltins: Set[String] = Set(
    "String",
    "Int",
    "Long",
    "Double",
    "Float",
    "Boolean",
    "Byte",
    "Short",
    "Char",
    "Unit",
    "BigInt",
    "BigDecimal",
    "Any",
    "AnyRef",
    "AnyVal",
    "Nothing",
    "Null"
  )

  private def fqn(ownerPkg: String, tpeOrTerm: String): String =
    if (tpeOrTerm.contains(".") || scalaBuiltins.contains(tpeOrTerm)) tpeOrTerm
    else s"$ownerPkg.$tpeOrTerm"

}
