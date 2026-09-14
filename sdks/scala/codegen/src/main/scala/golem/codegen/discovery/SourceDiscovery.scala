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

package golem.codegen.discovery

import scala.meta._
import scala.meta.parsers._

import java.security.MessageDigest

/**
 * Shared source discovery module for Golem codegen.
 *
 * Scans Scala source text using scalameta for:
 *   - `@agentDefinition` annotated traits
 *   - `@agentImplementation` annotated classes
 *   - `@toolDefinition` annotated traits
 *   - `@toolImplementation` annotated classes
 *   - `@toolMiddleware` annotated classes
 *   - `@universalToolMiddleware` annotated classes
 *   - Top-level objects (for companion conflict detection)
 *
 * All results are returned as pure data; no code generation is performed here.
 */
object SourceDiscovery {

  final case class SourceInput(path: String, content: String)

  final case class Warning(path: Option[String], message: String)

  final case class Error(path: Option[String], message: String)

  /** A non-secret config field discovered from a config case class. */
  final case class ConfigField(
    path: List[String],
    typeExpr: String
  )

  /** Discovered `@agentDefinition` trait. */
  final case class AgentTrait(
    path: String,
    pkg: String,
    name: String,
    typeName: Option[String],
    constructorParams: List[ConstructorParam],
    hasDescription: Boolean,
    descriptionValue: Option[String],
    mode: Option[String],
    methods: List[DiscoveredMethod],
    configFields: List[ConfigField] = Nil
  )

  final case class ConstructorParam(name: String, typeExpr: String)

  final case class DiscoveredMethod(
    name: String,
    params: List[ConstructorParam],
    returnTypeExpr: String,
    principalParams: List[Boolean]
  )

  /** Discovered `@agentImplementation` class. */
  final case class AgentImpl(
    pkg: String,
    implClass: String,
    traitType: String,
    ctorTypes: List[String],
    parentTypes: List[String],
    imports: Map[String, String],
    wildcardImports: List[WildcardImport]
  )

  final case class WildcardImport(pkg: String, excludes: Set[String])

  /** Discovered `@toolImplementation` class. */
  final case class ToolImpl(
    pkg: String,
    implClass: String,
    traitType: String,
    parentTypes: List[String],
    imports: Map[String, String],
    wildcardImports: List[WildcardImport]
  )

  /** Discovered `@toolMiddleware` implementation class. */
  final case class ToolMiddlewareImpl(
    path: String,
    pkg: String,
    implClass: String,
    middlewareName: String,
    aliases: List[String],
    description: Option[String],
    presentedToolType: String,
    expectedToolType: String,
    transparent: Boolean,
    parentType: String,
    imports: Map[String, String],
    wildcardImports: List[WildcardImport],
    sourceHash: String,
    surfaceHash: String
  )

  /** Discovered `@universalToolMiddleware` implementation class. */
  final case class UniversalToolMiddlewareImpl(
    path: String,
    pkg: String,
    implClass: String,
    middlewareName: String,
    aliases: List[String],
    description: Option[String],
    parentType: String,
    imports: Map[String, String],
    wildcardImports: List[WildcardImport],
    sourceHash: String,
    surfaceHash: String
  )

  /** One `@arg(...)` annotation on a tool trait method. */
  final case class ToolArgAnnotation(
    name: String,
    aliases: List[String],
    scope: Option[String],
    kind: Option[String],
    syntax: String
  )

  /** One method of a discovered `@toolDefinition` trait. */
  final case class ToolMethod(
    name: String,
    params: List[ConstructorParam],
    returnTypeExpr: String,
    commandName: Option[String],
    commandAliases: List[String],
    args: List[ToolArgAnnotation],
    resultAnnotations: List[String],
    constraintAnnotations: List[String],
    commandAnnotations: List[String]
  )

  /** Discovered `@toolDefinition` trait. */
  final case class ToolTrait(
    path: String,
    pkg: String,
    name: String,
    toolName: Option[String],
    version: Option[String],
    sourceHash: String,
    methods: List[ToolMethod],
    imports: Map[String, String],
    wildcardImports: List[WildcardImport],
    knownTypeFqns: Set[String] = Set.empty
  )

  /** Discovered top-level object (for companion conflict detection). */
  final case class ExistingObject(path: String, pkg: String, name: String)

  final case class Result(
    traits: Seq[AgentTrait],
    implementations: Seq[AgentImpl],
    toolImplementations: Seq[ToolImpl],
    objects: Seq[ExistingObject],
    warnings: Seq[Warning],
    tools: Seq[ToolTrait] = Nil,
    sourceHashes: Seq[(String, String)] = Nil,
    toolMiddlewares: Seq[ToolMiddlewareImpl] = Nil,
    universalToolMiddlewares: Seq[UniversalToolMiddlewareImpl] = Nil,
    errors: Seq[Error] = Nil
  )

  /**
   * Discover all agent traits, implementations, and top-level objects from the
   * given sources.
   */
  def discover(sources: Seq[SourceInput]): Result = {
    val warnings  = List.newBuilder[Warning]
    val traits    = List.newBuilder[AgentTrait]
    val impls     = List.newBuilder[AgentImpl]
    val toolImpls = List.newBuilder[ToolImpl]
    val objects   = List.newBuilder[ExistingObject]
    val tools     = List.newBuilder[ToolTrait]

    val parsedTrees: Seq[(SourceInput, Tree)] = sources.flatMap { src =>
      parseSource(src.content) match {
        case Some(tree) => Some((src, tree))
        case None       =>
          warnings += Warning(Some(src.path), "Failed to parse source file.")
          None
      }
    }

    // Build an index of case class definitions across all sources for config field extraction
    val caseClassIndex: Map[String, (String, Defn.Class)] = {
      val builder = Map.newBuilder[String, (String, Defn.Class)]
      parsedTrees.foreach { case (_, tree) =>
        collectCaseClasses(tree, "", builder)
      }
      builder.result()
    }

    val knownTypeFqns: Set[String] = {
      val builder = Set.newBuilder[String]
      parsedTrees.foreach { case (_, tree) =>
        collectDeclaredTypes(tree, "", builder)
      }
      builder.result()
    }

    parsedTrees.foreach { case (src, tree) =>
      collect(
        tree,
        "",
        Map.empty,
        Nil,
        src.path,
        sourceHash(src.content),
        warnings,
        traits,
        impls,
        toolImpls,
        objects,
        tools,
        caseClassIndex
      )
    }

    val discoveredTools = tools
      .result()
      .distinct
      .map(_.copy(knownTypeFqns = knownTypeFqns))
      .sortBy(t => (t.pkg, t.name))
    val middlewares = discoverMiddlewares(parsedTrees, discoveredTools)

    Result(
      traits = traits.result().distinct.sortBy(t => (t.pkg, t.name)),
      implementations = impls.result().distinct.sortBy(ai => (ai.pkg, ai.traitType, ai.implClass)),
      toolImplementations = toolImpls.result().distinct.sortBy(ti => (ti.pkg, ti.traitType, ti.implClass)),
      objects = objects.result().distinct.sortBy(o => (o.pkg, o.name)),
      warnings = warnings.result(),
      tools = discoveredTools,
      sourceHashes = parsedTrees.map { case (src, _) => src.path -> sourceHash(src.content) }.sortBy(_._1),
      toolMiddlewares = middlewares.toolMiddlewares,
      universalToolMiddlewares = middlewares.universalToolMiddlewares,
      errors = middlewares.errors
    )
  }

  // ── Parsing ────────────────────────────────────────────────────────────────

  private def parseSource(source: String): Option[Source] =
    dialects.Scala3(source).parse[Source].toOption

  private def collectDeclaredTypes(
    tree: Tree,
    pkg: String,
    builder: scala.collection.mutable.Builder[String, Set[String]]
  ): Unit = {
    def add(name: String): Unit =
      builder += qualifiedName(pkg, name)

    tree match {
      case source: Source =>
        source.stats.foreach(collectDeclaredTypes(_, pkg, builder))
      case pkgNode: Pkg =>
        pkgNode.stats.foreach(collectDeclaredTypes(_, appendPkg(pkg, pkgNode.ref.syntax), builder))
      case Pkg.Object(_, name, templ) =>
        add(name.value)
        templ.stats.foreach(collectDeclaredTypes(_, appendPkg(pkg, name.value), builder))
      case definition: Defn.Class  => add(definition.name.value)
      case definition: Defn.Trait  => add(definition.name.value)
      case definition: Defn.Object => add(definition.name.value)
      case definition: Defn.Enum   => add(definition.name.value)
      case definition: Defn.Type   => add(definition.name.value)
      case declaration: Decl.Type  => add(declaration.name.value)
      case _                       => ()
    }
  }

  private def sourceHash(source: String): String = {
    val digest = MessageDigest.getInstance("SHA-256")
    digest.digest(source.getBytes("UTF-8")).map(b => f"$b%02x").mkString
  }

  // ── Annotation detection ───────────────────────────────────────────────────

  private def hasAnnotation(mods: List[Mod], annotName: String): Boolean =
    mods.exists {
      case Mod.Annot(init) =>
        val full = init.tpe.syntax
        full == annotName || full.endsWith(s".$annotName")
      case _ => false
    }

  private def hasAgentDefinition(mods: List[Mod]): Boolean =
    hasAnnotation(mods, "agentDefinition")

  private def hasAgentImplementation(mods: List[Mod]): Boolean =
    hasAnnotation(mods, "agentImplementation")

  private def hasToolDefinition(mods: List[Mod]): Boolean =
    hasAnnotation(mods, "toolDefinition")

  private def hasToolImplementation(mods: List[Mod]): Boolean =
    hasAnnotation(mods, "toolImplementation")

  private def hasToolMiddleware(mods: List[Mod]): Boolean =
    hasAnnotation(mods, "toolMiddleware")

  private def hasUniversalToolMiddleware(mods: List[Mod]): Boolean =
    hasAnnotation(mods, "universalToolMiddleware")

  /** Flatten all annotation arguments from an Init node. */
  private def flattenArgs(init: Init): List[Term] =
    init.argClauses.toList.flatMap(_.values)

  /**
   * Extract `typeName` from `@agentDefinition(typeName = "Foo")` or
   * `@agentDefinition("Foo")`.
   */
  private def extractTypeName(mods: List[Mod]): Option[String] =
    mods.collectFirst {
      case Mod.Annot(init) if {
            val full = init.tpe.syntax
            full == "agentDefinition" || full.endsWith(".agentDefinition")
          } =>
        init
    }.flatMap { init =>
      val args = flattenArgs(init)
      // Named argument: @agentDefinition(typeName = "Foo")
      val named = args.collectFirst {
        case Term.Assign(Term.Name("typeName"), Lit.String(v)) if v.nonEmpty => v
      }
      named.orElse {
        // Positional first argument: @agentDefinition("Foo")
        args.headOption.collect {
          case Lit.String(v) if v.nonEmpty => v
        }
      }
    }

  /** Extract `@description("...")` value from a trait's modifiers. */
  private def extractDescription(mods: List[Mod]): (Boolean, Option[String]) =
    mods.collectFirst {
      case Mod.Annot(init) if {
            val full = init.tpe.syntax
            full == "description" || full.endsWith(".description")
          } =>
        init
    } match {
      case Some(init) =>
        val value = flattenArgs(init).headOption.collect { case Lit.String(v) =>
          v
        }
        (true, value)
      case None =>
        (false, None)
    }

  /**
   * Extract `mode` from `@agentDefinition(mode = DurabilityMode.Ephemeral)`.
   */
  private def extractMode(mods: List[Mod]): Option[String] =
    mods.collectFirst {
      case Mod.Annot(init) if {
            val full = init.tpe.syntax
            full == "agentDefinition" || full.endsWith(".agentDefinition")
          } =>
        init
    }.flatMap { init =>
      val args = flattenArgs(init)
      // Named argument: mode = DurabilityMode.Ephemeral
      val named = args.collectFirst { case Term.Assign(Term.Name("mode"), term) =>
        extractModeValue(term)
      }.flatten
      named.orElse {
        // Positional second argument (index 1)
        args.lift(1).flatMap(extractModeValue)
      }
    }

  private def extractModeValue(term: Term): Option[String] =
    term match {
      case Term.Select(_, Term.Name("Ephemeral")) => Some("ephemeral")
      case Term.Select(_, Term.Name("Durable"))   => Some("durable")
      case Term.Name("Ephemeral")                 => Some("ephemeral")
      case Term.Name("Durable")                   => Some("durable")
      case _                                      => None
    }

  /**
   * Extract constructor parameters from the id schema class in a trait body.
   * Looks first for a class annotated with `@id`, then falls back to a class
   * named `Id`.
   */
  private def extractConstructorParams(templ: Template): List[ConstructorParam] = {
    def paramsFromClass(d: Defn.Class): List[ConstructorParam] =
      d.ctor.paramClauses
        .flatMap(_.values)
        .flatMap { param =>
          param.decltpe.map(tpe => ConstructorParam(param.name.value, tpe.syntax))
        }
        .toList

    val annotated = templ.stats.collectFirst {
      case d: Defn.Class if hasAnnotation(d.mods, "id") => paramsFromClass(d)
    }

    annotated.getOrElse {
      templ.stats.collectFirst {
        case d: Defn.Class if d.name.value == "Id" => paramsFromClass(d)
      }.getOrElse(Nil)
    }
  }

  /** Check if a type AST represents `Principal` or `golem.Principal`. */
  private def isPrincipalType(tpe: Type): Boolean =
    tpe match {
      case Type.Name("Principal")                 => true
      case Type.Select(_, Type.Name("Principal")) => true
      case _                                      => false
    }

  /** Extract non-constructor methods from a trait body. */
  private def extractMethods(templ: Template): List[DiscoveredMethod] =
    templ.stats.flatMap {
      case d: Decl.Def =>
        val params = d.paramClauseGroups.flatMap(_.paramClauses).flatMap(_.values).flatMap { param =>
          param.decltpe.map(tpe => (ConstructorParam(param.name.value, tpe.syntax), isPrincipalType(tpe)))
        }
        Some(
          DiscoveredMethod(
            name = d.name.value,
            params = params.map(_._1),
            returnTypeExpr = d.decltpe.syntax,
            principalParams = params.map(_._2)
          )
        )
      case d: Defn.Def =>
        d.decltpe match {
          case Some(retTpe) =>
            val params = d.paramClauseGroups.flatMap(_.paramClauses).flatMap(_.values).flatMap { param =>
              param.decltpe.map(tpe => (ConstructorParam(param.name.value, tpe.syntax), isPrincipalType(tpe)))
            }
            Some(
              DiscoveredMethod(
                name = d.name.value,
                params = params.map(_._1),
                returnTypeExpr = retTpe.syntax,
                principalParams = params.map(_._2)
              )
            )
          case None => None // Skip methods with no explicit return type
        }
      case _ => None
    }.toList

  // ── Tool trait extraction ─────────────────────────────────────────────────

  private def annotationInits(mods: List[Mod], annotName: String): List[Init] =
    mods.collect {
      case Mod.Annot(init) if {
            val full = init.tpe.syntax
            full == annotName || full.endsWith(s".$annotName")
          } =>
        init
    }

  /** Extract a string literal from a term (plain literal only). */
  private def stringLit(term: Term): Option[String] =
    term match {
      case Lit.String(v) => Some(v)
      case _             => None
    }

  /**
   * Extract string entries from an `Array(...)`/`List(...)`/`Seq(...)` literal.
   */
  private def stringArrayTerm(term: Term): List[String] =
    term match {
      case apply: Term.Apply =>
        apply.argClause.values.collect { case Lit.String(v) => v }
      case _ => Nil
    }

  private def namedArg(args: List[Term], name: String): Option[Term] =
    args.collectFirst { case Term.Assign(Term.Name(`name`), value) => value }

  /**
   * Extract the tool name from `@toolDefinition(name = "x")` /
   * `@toolDefinition("x")`.
   */
  private def extractToolName(mods: List[Mod]): Option[String] =
    annotationInits(mods, "toolDefinition").headOption.flatMap { init =>
      val args = flattenArgs(init)
      namedArg(args, "name")
        .flatMap(stringLit)
        .orElse(args.headOption.flatMap(stringLit))
        .filter(_.nonEmpty)
    }

  /** Extract the version from `@toolDefinition(version = "x")`. */
  private def extractToolVersion(mods: List[Mod]): Option[String] =
    annotationInits(mods, "toolDefinition").headOption.flatMap { init =>
      val args = flattenArgs(init)
      namedArg(args, "version")
        .flatMap(stringLit)
        .orElse(args.lift(1).flatMap(stringLit))
        .filter(_.nonEmpty)
    }

  /** Extract `@command(name, aliases)` from a method's modifiers. */
  private def extractCommand(mods: List[Mod]): (Option[String], List[String]) =
    annotationInits(mods, "command").headOption match {
      case None       => (None, Nil)
      case Some(init) =>
        val args = flattenArgs(init)
        val name = namedArg(args, "name")
          .flatMap(stringLit)
          .orElse(args.headOption.flatMap(stringLit))
          .filter(_.nonEmpty)
        val aliases = namedArg(args, "aliases")
          .map(stringArrayTerm)
          .orElse(args.lift(1).map(stringArrayTerm))
          .getOrElse(Nil)
        (name, aliases)
    }

  /**
   * Extract the `@arg(...)` annotations of a method (surface name, aliases,
   * scope, kind).
   */
  private def extractArgs(mods: List[Mod]): List[ToolArgAnnotation] =
    annotationInits(mods, "arg").flatMap { init =>
      val args = flattenArgs(init)
      val name = namedArg(args, "name")
        .flatMap(stringLit)
        .orElse(args.headOption.flatMap(stringLit))
      name.map { n =>
        ToolArgAnnotation(
          name = n,
          aliases = namedArg(args, "aliases").map(stringArrayTerm).getOrElse(Nil),
          scope = namedArg(args, "scope").flatMap(stringLit).filter(_.nonEmpty),
          kind = namedArg(args, "kind").flatMap(stringLit).filter(_.nonEmpty),
          syntax = init.syntax
        )
      }
    }

  /**
   * Extract the declared methods of a tool trait with their tool annotations.
   */
  private def extractToolMethods(templ: Template): List[ToolMethod] = {
    def method(
      name: String,
      mods: List[Mod],
      paramss: List[Term.Param],
      retTpe: Type
    ): ToolMethod = {
      val params                 = paramss.flatMap(p => p.decltpe.map(tpe => ConstructorParam(p.name.value, tpe.syntax)))
      val (commandName, aliases) = extractCommand(mods)
      ToolMethod(
        name = name,
        params = params,
        returnTypeExpr = retTpe.syntax,
        commandName = commandName,
        commandAliases = aliases,
        args = extractArgs(mods),
        resultAnnotations = annotationInits(mods, "result").map(_.syntax),
        constraintAnnotations = annotationInits(mods, "constraint").map(_.syntax),
        commandAnnotations = annotationInits(mods, "annotations").map(_.syntax)
      )
    }

    templ.stats.flatMap {
      case d: Decl.Def =>
        Some(
          method(
            d.name.value,
            d.mods,
            d.paramClauseGroups.flatMap(_.paramClauses).flatMap(_.values),
            d.decltpe
          )
        )
      case d: Defn.Def =>
        d.decltpe.map { retTpe =>
          method(
            d.name.value,
            d.mods,
            d.paramClauseGroups.flatMap(_.paramClauses).flatMap(_.values),
            retTpe
          )
        }
      case _ => None
    }.toList
  }

  // ── AST walking ────────────────────────────────────────────────────────────

  private def appendPkg(prefix: String, name: String): String =
    if (prefix.isEmpty) name else s"$prefix.$name"

  private def collectStats(
    stats: Iterable[Stat],
    pkg: String,
    imports: Map[String, String],
    wildcardImports: List[WildcardImport],
    sourcePath: String,
    sourceHash: String,
    warnings: scala.collection.mutable.Builder[Warning, List[Warning]],
    traits: scala.collection.mutable.Builder[AgentTrait, List[AgentTrait]],
    impls: scala.collection.mutable.Builder[AgentImpl, List[AgentImpl]],
    toolImpls: scala.collection.mutable.Builder[ToolImpl, List[ToolImpl]],
    objects: scala.collection.mutable.Builder[ExistingObject, List[ExistingObject]],
    tools: scala.collection.mutable.Builder[ToolTrait, List[ToolTrait]],
    caseClassIndex: Map[String, (String, Defn.Class)]
  ): Unit = {
    var visibleImports         = imports
    var visibleWildcardImports = wildcardImports
    stats.foreach {
      case i: Import =>
        visibleImports = visibleImports ++ extractNamedImports(i)
        visibleWildcardImports = visibleWildcardImports ++ extractWildcardImports(i)
      case stat =>
        collect(
          stat,
          pkg,
          visibleImports,
          visibleWildcardImports,
          sourcePath,
          sourceHash,
          warnings,
          traits,
          impls,
          toolImpls,
          objects,
          tools,
          caseClassIndex
        )
    }
  }

  private def extractNamedImports(importStat: Import): Map[String, String] =
    importStat.importers.flatMap { importer =>
      importer.importees.collect {
        case Importee.Name(name) =>
          name.value -> s"${importer.ref.syntax}.${name.value}"
        case Importee.Rename(name, rename) =>
          rename.value -> s"${importer.ref.syntax}.${name.value}"
      }
    }.toMap

  private def extractWildcardImports(importStat: Import): List[WildcardImport] =
    importStat.importers.flatMap { importer =>
      val hasWildcard = importer.importees.exists(_.isInstanceOf[Importee.Wildcard])
      if (hasWildcard) {
        val excludes = importer.importees.collect {
          case Importee.Unimport(name)  => name.value
          case Importee.Rename(name, _) => name.value
        }.toSet
        List(WildcardImport(importer.ref.syntax, excludes))
      } else Nil
    }

  private def collect(
    tree: Tree,
    pkg: String,
    imports: Map[String, String],
    wildcardImports: List[WildcardImport],
    sourcePath: String,
    sourceHash: String,
    warnings: scala.collection.mutable.Builder[Warning, List[Warning]],
    traits: scala.collection.mutable.Builder[AgentTrait, List[AgentTrait]],
    impls: scala.collection.mutable.Builder[AgentImpl, List[AgentImpl]],
    toolImpls: scala.collection.mutable.Builder[ToolImpl, List[ToolImpl]],
    objects: scala.collection.mutable.Builder[ExistingObject, List[ExistingObject]],
    tools: scala.collection.mutable.Builder[ToolTrait, List[ToolTrait]],
    caseClassIndex: Map[String, (String, Defn.Class)]
  ): Unit =
    tree match {
      case source: Source =>
        collectStats(
          source.stats,
          pkg,
          imports,
          wildcardImports,
          sourcePath,
          sourceHash,
          warnings,
          traits,
          impls,
          toolImpls,
          objects,
          tools,
          caseClassIndex
        )

      case pkgNode: Pkg =>
        val nextPkg = appendPkg(pkg, pkgNode.ref.syntax)
        collectStats(
          pkgNode.stats,
          nextPkg,
          imports,
          wildcardImports,
          sourcePath,
          sourceHash,
          warnings,
          traits,
          impls,
          toolImpls,
          objects,
          tools,
          caseClassIndex
        )

      case Pkg.Object(_, name, templ) =>
        val nextPkg = appendPkg(pkg, name.value)
        collectStats(
          templ.stats,
          nextPkg,
          imports,
          wildcardImports,
          sourcePath,
          sourceHash,
          warnings,
          traits,
          impls,
          toolImpls,
          objects,
          tools,
          caseClassIndex
        )

      case t: Defn.Trait if hasToolDefinition(t.mods) =>
        tools += ToolTrait(
          path = sourcePath,
          pkg = pkg,
          name = t.name.value,
          toolName = extractToolName(t.mods),
          version = extractToolVersion(t.mods),
          sourceHash = sourceHash,
          methods = extractToolMethods(t.templ),
          imports = imports,
          wildcardImports = wildcardImports
        )

      case t: Defn.Trait if hasAgentDefinition(t.mods) =>
        val typeName           = extractTypeName(t.mods)
        val (hasDesc, descVal) = extractDescription(t.mods)
        val ctorParams         = extractConstructorParams(t.templ)
        val modeValue          = extractMode(t.mods)
        val discoveredMethods  = extractMethods(t.templ)
        val cfgFields          = extractAgentConfigType(t.templ, pkg)
          .flatMap(cfgType => extractConfigFields(cfgType, pkg, caseClassIndex, Nil))
          .getOrElse(Nil)
        traits += AgentTrait(
          path = sourcePath,
          pkg = pkg,
          name = t.name.value,
          typeName = typeName,
          constructorParams = ctorParams,
          hasDescription = hasDesc,
          descriptionValue = descVal,
          mode = modeValue,
          methods = discoveredMethods,
          configFields = cfgFields
        )

      case cls: Defn.Class if hasAgentImplementation(cls.mods) =>
        val parentTypes                  = cls.templ.inits.map(_.tpe.syntax).toList
        val traitTypeOpt: Option[String] = parentTypes.headOption
        val ctorParams                   = cls.ctor.paramClauses.flatMap(_.values)
        val ctorTypes: List[String]      = ctorParams.map(_.decltpe.map(_.syntax).getOrElse("")).toList
        traitTypeOpt match {
          case Some(traitType) if pkg.nonEmpty && !ctorTypes.exists(_.isEmpty) =>
            impls += AgentImpl(
              pkg = pkg,
              implClass = cls.name.value,
              traitType = traitType,
              ctorTypes = ctorTypes,
              parentTypes = parentTypes,
              imports = imports,
              wildcardImports = wildcardImports
            )
          case _ =>
            if (ctorTypes.exists(_.isEmpty))
              warnings += Warning(
                Some(sourcePath),
                s"Skipping @agentImplementation ${cls.name.value} (missing constructor type annotations)."
              )
        }

      case cls: Defn.Class if hasToolImplementation(cls.mods) =>
        val parentTypes = cls.templ.inits.map(_.tpe.syntax).toList
        parentTypes.headOption match {
          case Some(traitType) if pkg.nonEmpty =>
            toolImpls += ToolImpl(
              pkg = pkg,
              implClass = cls.name.value,
              traitType = traitType,
              parentTypes = parentTypes,
              imports = imports,
              wildcardImports = wildcardImports
            )
          case _ =>
            warnings += Warning(
              Some(sourcePath),
              s"Skipping @toolImplementation ${cls.name.value} (missing implemented tool trait)."
            )
        }

      case obj: Defn.Object =>
        objects += ExistingObject(
          path = sourcePath,
          pkg = pkg,
          name = obj.name.value
        )

      case _ =>
        ()
    }

  // ── Tool middleware extraction ────────────────────────────────────────────

  private final case class MiddlewareDiscovery(
    toolMiddlewares: Seq[ToolMiddlewareImpl],
    universalToolMiddlewares: Seq[UniversalToolMiddlewareImpl],
    errors: Seq[Error]
  )

  private final case class GeneratedToolRef(
    toolType: String,
    generatedSimpleName: String,
    allowWildcardResolution: Boolean
  )

  private final case class ParsedMiddlewareParent(
    presented: GeneratedToolRef,
    expected: GeneratedToolRef,
    transparent: Boolean,
    syntax: String
  )

  private def discoverMiddlewares(
    parsedTrees: Seq[(SourceInput, Tree)],
    tools: Seq[ToolTrait]
  ): MiddlewareDiscovery = {
    val toolMiddlewares      = List.newBuilder[ToolMiddlewareImpl]
    val universalMiddlewares = List.newBuilder[UniversalToolMiddlewareImpl]
    val errors               = List.newBuilder[Error]

    parsedTrees.foreach { case (source, tree) =>
      collectMiddlewareTree(
        tree,
        pkg = "",
        imports = Map.empty,
        wildcardImports = Nil,
        sourcePath = source.path,
        sourceHash = sourceHash(source.content),
        tools = tools,
        toolMiddlewares = toolMiddlewares,
        universalMiddlewares = universalMiddlewares,
        errors = errors
      )
    }

    val monomorphic     = toolMiddlewares.result().distinct.sortBy(m => (m.middlewareName, m.pkg, m.implClass))
    val universal       = universalMiddlewares.result().distinct.sortBy(m => (m.middlewareName, m.pkg, m.implClass))
    val duplicateErrors = (monomorphic.map(m => (m.middlewareName, m.path, m.pkg, m.implClass)) ++
      universal.map(m => (m.middlewareName, m.path, m.pkg, m.implClass)))
      .groupBy(_._1)
      .toSeq
      .sortBy(_._1)
      .collect {
        case (name, duplicates) if duplicates.size > 1 =>
          val locations = duplicates.sortBy { case (_, path, pkg, implClass) => (path, pkg, implClass) }.map {
            case (_, path, pkg, implClass) =>
              val fqn = if (pkg.isEmpty) implClass else s"$pkg.$implClass"
              s"$fqn ($path)"
          }
            .mkString(", ")
          Error(None, s"Duplicate tool middleware name `$name`: $locations.")
      }

    MiddlewareDiscovery(
      toolMiddlewares = monomorphic,
      universalToolMiddlewares = universal,
      errors = (errors.result() ++ duplicateErrors).sortBy(error => (error.path.getOrElse(""), error.message))
    )
  }

  private def collectMiddlewareStats(
    stats: Iterable[Stat],
    pkg: String,
    imports: Map[String, String],
    wildcardImports: List[WildcardImport],
    sourcePath: String,
    sourceHash: String,
    tools: Seq[ToolTrait],
    toolMiddlewares: scala.collection.mutable.Builder[ToolMiddlewareImpl, List[ToolMiddlewareImpl]],
    universalMiddlewares: scala.collection.mutable.Builder[UniversalToolMiddlewareImpl, List[
      UniversalToolMiddlewareImpl
    ]],
    errors: scala.collection.mutable.Builder[Error, List[Error]]
  ): Unit = {
    var visibleImports         = imports
    var visibleWildcardImports = wildcardImports
    stats.foreach {
      case importStat: Import =>
        visibleImports = visibleImports ++ extractNamedImports(importStat)
        visibleWildcardImports = visibleWildcardImports ++ extractWildcardImports(importStat)
      case stat =>
        collectMiddlewareTree(
          stat,
          pkg,
          visibleImports,
          visibleWildcardImports,
          sourcePath,
          sourceHash,
          tools,
          toolMiddlewares,
          universalMiddlewares,
          errors
        )
    }
  }

  private def collectMiddlewareTree(
    tree: Tree,
    pkg: String,
    imports: Map[String, String],
    wildcardImports: List[WildcardImport],
    sourcePath: String,
    sourceHash: String,
    tools: Seq[ToolTrait],
    toolMiddlewares: scala.collection.mutable.Builder[ToolMiddlewareImpl, List[ToolMiddlewareImpl]],
    universalMiddlewares: scala.collection.mutable.Builder[UniversalToolMiddlewareImpl, List[
      UniversalToolMiddlewareImpl
    ]],
    errors: scala.collection.mutable.Builder[Error, List[Error]]
  ): Unit =
    tree match {
      case source: Source =>
        collectMiddlewareStats(
          source.stats,
          pkg,
          imports,
          wildcardImports,
          sourcePath,
          sourceHash,
          tools,
          toolMiddlewares,
          universalMiddlewares,
          errors
        )
      case pkgNode: Pkg =>
        collectMiddlewareStats(
          pkgNode.stats,
          appendPkg(pkg, pkgNode.ref.syntax),
          imports,
          wildcardImports,
          sourcePath,
          sourceHash,
          tools,
          toolMiddlewares,
          universalMiddlewares,
          errors
        )
      case Pkg.Object(_, name, templ) =>
        collectMiddlewareStats(
          templ.stats,
          appendPkg(pkg, name.value),
          imports,
          wildcardImports,
          sourcePath,
          sourceHash,
          tools,
          toolMiddlewares,
          universalMiddlewares,
          errors
        )
      case cls: Defn.Class if hasToolMiddleware(cls.mods) && hasUniversalToolMiddleware(cls.mods) =>
        errors += Error(
          Some(sourcePath),
          s"Tool middleware `${qualifiedName(pkg, cls.name.value)}` cannot use both @toolMiddleware and @universalToolMiddleware."
        )
      case cls: Defn.Class if hasToolMiddleware(cls.mods) =>
        discoverToolMiddlewareClass(
          cls,
          pkg,
          imports,
          wildcardImports,
          sourcePath,
          sourceHash,
          tools,
          toolMiddlewares,
          errors
        )
      case cls: Defn.Class if hasUniversalToolMiddleware(cls.mods) =>
        discoverUniversalToolMiddlewareClass(
          cls,
          pkg,
          imports,
          wildcardImports,
          sourcePath,
          sourceHash,
          universalMiddlewares,
          errors
        )
      case traitDef: Defn.Trait if hasToolMiddleware(traitDef.mods) || hasUniversalToolMiddleware(traitDef.mods) =>
        errors += Error(
          Some(sourcePath),
          s"Tool middleware `${qualifiedName(pkg, traitDef.name.value)}` must be a concrete class, not a trait."
        )
      case objectDef: Defn.Object if hasToolMiddleware(objectDef.mods) || hasUniversalToolMiddleware(objectDef.mods) =>
        errors += Error(
          Some(sourcePath),
          s"Tool middleware `${qualifiedName(pkg, objectDef.name.value)}` must be a concrete class, not an object."
        )
      case _ => ()
    }

  private def discoverToolMiddlewareClass(
    cls: Defn.Class,
    pkg: String,
    imports: Map[String, String],
    wildcardImports: List[WildcardImport],
    sourcePath: String,
    sourceHash: String,
    tools: Seq[ToolTrait],
    toolMiddlewares: scala.collection.mutable.Builder[ToolMiddlewareImpl, List[ToolMiddlewareImpl]],
    errors: scala.collection.mutable.Builder[Error, List[Error]]
  ): Unit = {
    val className  = qualifiedName(pkg, cls.name.value)
    val valid      = validateMiddlewareClass(cls, className, sourcePath, errors)
    val annotation = extractMiddlewareAnnotation(cls.mods, "toolMiddleware")
    val name       = annotation.flatMap(_._1).filter(_.trim.nonEmpty)
    val aliases    = annotation.map(_._2).getOrElse(Nil)
    if (name.isEmpty)
      errors += Error(Some(sourcePath), s"Tool middleware `$className` must declare a non-empty name.")

    val parentCandidates = cls.templ.inits.flatMap { init =>
      parseMiddlewareParent(init.tpe, imports).map(parent => parent.copy(syntax = init.tpe.syntax))
    }
    val parent = parentCandidates match {
      case single :: Nil => Some(single)
      case Nil           =>
        errors += Error(
          Some(sourcePath),
          s"Tool middleware `$className` must directly extend a generated `<Tool>Middleware` or `<Tool>Middleware.Adapter[<Expected>Underlying]` trait."
        )
        None
      case _ =>
        errors += Error(
          Some(sourcePath),
          s"Tool middleware `$className` has multiple generated middleware parents: ${parentCandidates.map(_.syntax).mkString(", ")}."
        )
        None
    }

    val resolved = parent.flatMap { parsed =>
      val presented = resolveToolReference(parsed.presented, pkg, wildcardImports, tools)
      val expected  = resolveToolReference(parsed.expected, pkg, wildcardImports, tools)
      if (presented.isEmpty)
        errors += Error(
          Some(sourcePath),
          s"Tool middleware `$className` has an unresolved presented tool in parent `${parsed.syntax}`."
        )
      if (expected.isEmpty)
        errors += Error(
          Some(sourcePath),
          s"Tool middleware `$className` has an unresolved expected underlying in parent `${parsed.syntax}`."
        )
      for {
        presentedTool <- presented
        expectedTool  <- expected
      } yield (parsed, presentedTool, expectedTool)
    }

    for {
      middlewareName                        <- name
      (parsed, presentedTool, expectedTool) <- resolved
      if valid
    } {
      val description = extractDescription(cls.mods)._2
      val surface     =
        s"$className|name=$middlewareName|aliases=${aliases.mkString(",")}|description=${description.getOrElse("")}|presented=$presentedTool|expected=$expectedTool|transparent=${parsed.transparent}|parent=${parsed.syntax}|source=$sourceHash"
      toolMiddlewares += ToolMiddlewareImpl(
        path = sourcePath,
        pkg = pkg,
        implClass = cls.name.value,
        middlewareName = middlewareName,
        aliases = aliases,
        description = description,
        presentedToolType = presentedTool,
        expectedToolType = expectedTool,
        transparent = parsed.transparent,
        parentType = parsed.syntax,
        imports = imports,
        wildcardImports = wildcardImports,
        sourceHash = sourceHash,
        surfaceHash = sha256(surface)
      )
    }
  }

  private def discoverUniversalToolMiddlewareClass(
    cls: Defn.Class,
    pkg: String,
    imports: Map[String, String],
    wildcardImports: List[WildcardImport],
    sourcePath: String,
    sourceHash: String,
    universalMiddlewares: scala.collection.mutable.Builder[UniversalToolMiddlewareImpl, List[
      UniversalToolMiddlewareImpl
    ]],
    errors: scala.collection.mutable.Builder[Error, List[Error]]
  ): Unit = {
    val className  = qualifiedName(pkg, cls.name.value)
    val valid      = validateMiddlewareClass(cls, className, sourcePath, errors)
    val annotation = extractMiddlewareAnnotation(cls.mods, "universalToolMiddleware")
    val name       = annotation.flatMap(_._1).filter(_.trim.nonEmpty)
    val aliases    = annotation.map(_._2).getOrElse(Nil)
    if (name.isEmpty)
      errors += Error(Some(sourcePath), s"Universal tool middleware `$className` must declare a non-empty name.")

    val parents = cls.templ.inits.filter(init => isUniversalMiddlewareParent(init.tpe, imports, wildcardImports))
    val parent  = parents match {
      case single :: Nil => Some(single.tpe.syntax)
      case Nil           =>
        errors += Error(
          Some(sourcePath),
          s"Universal tool middleware `$className` must directly extend `golem.tool.UniversalToolMiddleware`."
        )
        None
      case _ =>
        errors += Error(
          Some(sourcePath),
          s"Universal tool middleware `$className` has multiple `UniversalToolMiddleware` parents."
        )
        None
    }

    for {
      middlewareName <- name
      parentType     <- parent
      if valid
    } {
      val description = extractDescription(cls.mods)._2
      val surface     =
        s"$className|name=$middlewareName|aliases=${aliases.mkString(",")}|description=${description.getOrElse("")}|parent=$parentType|source=$sourceHash"
      universalMiddlewares += UniversalToolMiddlewareImpl(
        path = sourcePath,
        pkg = pkg,
        implClass = cls.name.value,
        middlewareName = middlewareName,
        aliases = aliases,
        description = description,
        parentType = parentType,
        imports = imports,
        wildcardImports = wildcardImports,
        sourceHash = sourceHash,
        surfaceHash = sha256(surface)
      )
    }
  }

  private def validateMiddlewareClass(
    cls: Defn.Class,
    className: String,
    sourcePath: String,
    errors: scala.collection.mutable.Builder[Error, List[Error]]
  ): Boolean = {
    var valid = true
    if (cls.mods.exists(_.is[Mod.Abstract])) {
      errors += Error(Some(sourcePath), s"Tool middleware `$className` must be concrete, not abstract.")
      valid = false
    }
    if (cls.tparamClause.values.nonEmpty) {
      errors += Error(Some(sourcePath), s"Tool middleware `$className` must not declare type parameters.")
      valid = false
    }
    if (cls.ctor.paramClauses.flatMap(_.values).nonEmpty) {
      errors += Error(Some(sourcePath), s"Tool middleware `$className` must have a zero-argument primary constructor.")
      valid = false
    }
    if (cls.templ.stats.exists(_.isInstanceOf[Ctor.Secondary])) {
      errors += Error(Some(sourcePath), s"Tool middleware `$className` must not declare secondary constructors.")
      valid = false
    }
    valid
  }

  private def extractMiddlewareAnnotation(
    mods: List[Mod],
    annotationName: String
  ): Option[(Option[String], List[String])] =
    annotationInits(mods, annotationName).headOption.map { init =>
      val args = flattenArgs(init)
      val name = namedArg(args, "name")
        .flatMap(stringLit)
        .orElse(args.headOption.flatMap(stringLit))
      val aliases = namedArg(args, "aliases")
        .map(stringArrayTerm)
        .orElse(args.lift(1).map(stringArrayTerm))
        .getOrElse(Nil)
      (name, aliases)
    }

  private def parseMiddlewareParent(
    tpe: Type,
    imports: Map[String, String]
  ): Option[ParsedMiddlewareParent] =
    tpe match {
      case Type.Apply.After_4_6_0(Type.Select(presented, Type.Name("Adapter")), args) if args.values.size == 1 =>
        for {
          presentedTool <- generatedToolRef(presented.syntax, "Middleware", imports)
          expectedTool  <- generatedToolRef(args.values.head.syntax, "Underlying", imports)
        } yield ParsedMiddlewareParent(
          presented = presentedTool,
          expected = expectedTool,
          transparent = false,
          syntax = tpe.syntax
        )
      case _ =>
        generatedToolRef(tpe.syntax, "Middleware", imports).map { presented =>
          ParsedMiddlewareParent(
            presented = presented,
            expected = presented,
            transparent = true,
            syntax = tpe.syntax
          )
        }
    }

  private def generatedToolRef(
    rawRef: String,
    suffix: String,
    imports: Map[String, String]
  ): Option[GeneratedToolRef] = {
    val rooted     = rawRef.startsWith("_root_.")
    val normalized = normalizeTypeRef(rawRef)
    val expanded   = if (rooted) normalized else expandImportedTypeRef(normalized, imports)
    val simpleName = normalized.split('.').lastOption.getOrElse(normalized)
    if (!expanded.endsWith(suffix)) None
    else {
      val toolType = expanded.dropRight(suffix.length)
      if (toolType.isEmpty || toolType.endsWith(".")) None
      else
        Some(
          GeneratedToolRef(
            toolType = normalizeTypeRef(toolType),
            generatedSimpleName = simpleName,
            allowWildcardResolution = !rooted && !normalized.contains(".") && expanded == normalized
          )
        )
    }
  }

  private def resolveToolReference(
    ref: GeneratedToolRef,
    implPkg: String,
    wildcardImports: List[WildcardImport],
    tools: Seq[ToolTrait]
  ): Option[String] = {
    val byFqn          = tools.map(tool => qualifiedName(tool.pkg, tool.name) -> tool).toMap
    val simpleToolName = ref.toolType.split('.').lastOption.getOrElse(ref.toolType)

    val enclosingPackages = {
      val parts = implPkg.split('.').toList.filter(_.nonEmpty)
      parts.indices.reverse.map(index => parts.take(index + 1).mkString(".")).toList
    }
    val relativeCandidates =
      if (ref.toolType.contains(".")) enclosingPackages.map(prefix => s"$prefix.${ref.toolType}")
      else List(qualifiedName(implPkg, ref.toolType))
    val discovered = (relativeCandidates :+ ref.toolType).distinct.find(byFqn.contains)

    discovered.orElse {
      if (!ref.allowWildcardResolution) None
      else
        wildcardImports
          .filterNot(_.excludes.contains(ref.generatedSimpleName))
          .map(wildcard => qualifiedName(wildcard.pkg, simpleToolName))
          .distinct
          .filter(byFqn.contains) match {
          case single :: Nil => Some(single)
          case _             => None
        }
    }
  }

  private def isUniversalMiddlewareParent(
    tpe: Type,
    imports: Map[String, String],
    wildcardImports: List[WildcardImport]
  ): Boolean = {
    val raw        = tpe.syntax
    val rooted     = raw.startsWith("_root_.")
    val normalized = normalizeTypeRef(raw)
    val expanded   = if (rooted) normalized else expandImportedTypeRef(normalized, imports)
    expanded == "golem.tool.UniversalToolMiddleware" ||
    (expanded == "UniversalToolMiddleware" &&
      wildcardImports.exists(wildcard =>
        wildcard.pkg == "golem.tool" && !wildcard.excludes.contains("UniversalToolMiddleware")
      ))
  }

  private def expandImportedTypeRef(tpe: String, imports: Map[String, String]): String = {
    val dot = tpe.indexOf('.')
    if (dot < 0) imports.getOrElse(tpe, tpe)
    else {
      val qualifier = tpe.substring(0, dot)
      val rest      = tpe.substring(dot + 1)
      imports.get(qualifier).map(imported => s"$imported.$rest").getOrElse(tpe)
    }
  }

  private def normalizeTypeRef(tpe: String): String =
    tpe.stripPrefix("_root_.")

  private def qualifiedName(pkg: String, name: String): String =
    if (pkg.isEmpty) name else s"$pkg.$name"

  private def sha256(value: String): String =
    sourceHash(value)

  // ── Config field extraction ───────────────────────────────────────────────

  /**
   * Collect all case class definitions from the AST, indexed by simple name and
   * FQN.
   */
  private def collectCaseClasses(
    tree: Tree,
    pkg: String,
    builder: scala.collection.mutable.Builder[(String, (String, Defn.Class)), Map[String, (String, Defn.Class)]]
  ): Unit =
    tree match {
      case source: Source =>
        source.stats.foreach(collectCaseClasses(_, pkg, builder))
      case pkgNode: Pkg =>
        val nextPkg = appendPkg(pkg, pkgNode.ref.syntax)
        pkgNode.stats.foreach(collectCaseClasses(_, nextPkg, builder))
      case Pkg.Object(_, name, templ) =>
        val nextPkg = appendPkg(pkg, name.value)
        templ.stats.foreach(collectCaseClasses(_, nextPkg, builder))
      case cls: Defn.Class if cls.mods.exists(_.is[Mod.Case]) =>
        val fqn = if (pkg.isEmpty) cls.name.value else s"$pkg.${cls.name.value}"
        builder += (cls.name.value -> (pkg, cls))
        builder += (fqn            -> (pkg, cls))
      case _ => ()
    }

  /** Extract the type argument from `AgentConfig[T]` in a trait's parents. */
  private def extractAgentConfigType(templ: Template, currentPkg: String): Option[String] =
    templ.inits.collectFirst {
      case init if isAgentConfigType(init.tpe) =>
        extractTypeArg(init.tpe)
    }.flatten

  private def isAgentConfigType(tpe: Type): Boolean =
    tpe match {
      case Type.Apply.After_4_6_0(Type.Name("AgentConfig"), _)                 => true
      case Type.Apply.After_4_6_0(Type.Select(_, Type.Name("AgentConfig")), _) => true
      case _                                                                   => false
    }

  private def extractTypeArg(tpe: Type): Option[String] =
    tpe match {
      case Type.Apply.After_4_6_0(_, args) if args.size == 1 => Some(args.head.syntax)
      case _                                                 => None
    }

  /**
   * Extract non-secret config fields from a config type by looking up its case
   * class definition. Recursively flattens nested case classes.
   */
  private def extractConfigFields(
    typeName: String,
    currentPkg: String,
    caseClassIndex: Map[String, (String, Defn.Class)],
    path: List[String]
  ): Option[List[ConfigField]] = {
    // Try FQN first, then simple name
    val resolved = caseClassIndex
      .get(typeName)
      .orElse(caseClassIndex.get(if (currentPkg.isEmpty) typeName else s"$currentPkg.$typeName"))

    resolved.map { case (classPkg, cls) =>
      val params = cls.ctor.paramClauses.flatMap(_.values)
      params.flatMap { param =>
        val fieldName = param.name.value
        val fieldPath = path :+ fieldName
        param.decltpe match {
          case Some(tpe) if isSecretType(tpe) =>
            Nil // Skip secret fields
          case Some(tpe) =>
            val typeStr = tpe.syntax
            // Try to resolve as a nested config case class
            val nestedKey    = typeStr
            val nestedFqnKey = if (classPkg.isEmpty) typeStr else s"$classPkg.$typeStr"
            caseClassIndex.get(nestedKey).orElse(caseClassIndex.get(nestedFqnKey)) match {
              case Some(_) =>
                extractConfigFields(nestedKey, classPkg, caseClassIndex, fieldPath).getOrElse(Nil)
              case None =>
                List(ConfigField(fieldPath, typeStr))
            }
          case None => Nil
        }
      }.toList
    }
  }

  /** Check if a type AST represents `Secret[_]` or `golem.config.Secret[_]`. */
  private def isSecretType(tpe: Type): Boolean =
    tpe match {
      case Type.Apply.After_4_6_0(Type.Name("Secret"), _)                 => true
      case Type.Apply.After_4_6_0(Type.Select(_, Type.Name("Secret")), _) => true
      case _                                                              => false
    }
}
