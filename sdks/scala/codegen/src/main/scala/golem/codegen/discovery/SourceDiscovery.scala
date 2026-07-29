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
 *   - Top-level objects (for companion conflict detection)
 *
 * All results are returned as pure data; no code generation is performed here.
 */
object SourceDiscovery {

  final case class SourceInput(path: String, content: String)

  final case class Warning(path: Option[String], message: String)

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
    methods: List[ToolMethod]
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
    sourceHashes: Seq[(String, String)] = Nil
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

    Result(
      traits = traits.result().distinct.sortBy(t => (t.pkg, t.name)),
      implementations = impls.result().distinct.sortBy(ai => (ai.pkg, ai.traitType, ai.implClass)),
      toolImplementations = toolImpls.result().distinct.sortBy(ti => (ti.pkg, ti.traitType, ti.implClass)),
      objects = objects.result().distinct.sortBy(o => (o.pkg, o.name)),
      warnings = warnings.result(),
      tools = tools.result().distinct.sortBy(t => (t.pkg, t.name)),
      sourceHashes = parsedTrees.map { case (src, _) => src.path -> sourceHash(src.content) }.sortBy(_._1)
    )
  }

  // ── Parsing ────────────────────────────────────────────────────────────────

  private def parseSource(source: String): Option[Source] =
    dialects.Scala3(source).parse[Source].toOption

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
          methods = extractToolMethods(t.templ)
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
