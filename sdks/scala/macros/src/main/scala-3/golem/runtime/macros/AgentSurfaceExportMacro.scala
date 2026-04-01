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

package golem.runtime.macros

import scala.quoted.*

/**
 * Macro that exports agent surface metadata as a JSON string literal.
 *
 * This is used by the prepass compile phase to extract trait-level metadata
 * (type name, constructor params, description, mode, snapshotting) without
 * needing the full runtime. The JSON format matches
 * [[golem.codegen.ir.AgentSurfaceIR]].
 */
object AgentSurfaceExportMacro {

  inline def exportJson[T]: String =
    ${ exportJsonImpl[T] }

  private def exportJsonImpl[T: Type](using Quotes): Expr[String] = {
    import quotes.reflect.*

    val typeRepr   = TypeRepr.of[T]
    val typeSymbol = typeRepr.typeSymbol

    if !typeSymbol.flags.is(Flags.Trait) then
      report.errorAndAbort(s"AgentSurfaceExport target must be a trait, found: ${typeSymbol.fullName}")

    val agentDefinitionFQN       = "golem.runtime.annotations.agentDefinition"
    val descriptionAnnotationFQN = "golem.runtime.annotations.description"

    // Extract @agentDefinition annotation
    val annArgsOpt = typeSymbol.annotations.collectFirst {
      case Apply(Select(New(tpt), _), args) if tpt.tpe.dealias.typeSymbol.fullName == agentDefinitionFQN =>
        args
    }

    if annArgsOpt.isEmpty then
      report.errorAndAbort(s"Missing @agentDefinition(...) on agent trait: ${typeSymbol.fullName}")

    // Extract typeName
    val rawTypeName: String = annArgsOpt.flatMap { args =>
      args.collectFirst {
        case Literal(StringConstant(value)) if value.trim.nonEmpty                       => value
        case NamedArg("typeName", Literal(StringConstant(value))) if value.trim.nonEmpty => value
      }
    }.getOrElse(typeSymbol.name)

    // Extract description
    val description: Option[String] = typeSymbol.annotations.collectFirst {
      case Apply(Select(New(tpt), _), args) if tpt.tpe.dealias.typeSymbol.fullName == descriptionAnnotationFQN =>
        args.collectFirst { case Literal(StringConstant(value)) =>
          value
        }
    }.flatten

    // Extract mode from @agentDefinition
    val mode: String = annArgsOpt.flatMap { args =>
      val rawModeArg: Option[Term] =
        args.collectFirst { case NamedArg("mode", arg: Term) => arg }.orElse {
          args.lift(1).collect { case t: Term if !t.toString.contains("$default$") => t }
        }
      rawModeArg.flatMap {
        case Literal(StringConstant(value)) =>
          val v = value.trim.toLowerCase
          if (v.isEmpty) None else Some(v)
        case Select(_, name) if !name.contains("$") =>
          Some(name.toLowerCase)
        case Ident(name) if !name.contains("$") =>
          Some(name.toLowerCase)
        case _ => None
      }
    }.getOrElse("durable")

    // Extract snapshotting from @agentDefinition
    val snapshotting: String = extractStringArg(typeSymbol, agentDefinitionFQN, "snapshotting", 7)
      .getOrElse("disabled")

    // Extract constructor params from Id class
    val constructorParams: List[(String, String)] = {
      val idFQN = "golem.runtime.annotations.id"

      def hasIdAnnotation(sym: Symbol): Boolean =
        sym.annotations.exists {
          case Apply(Select(New(tpt), _), _) => tpt.tpe.dealias.typeSymbol.fullName == idFQN
          case _                             => false
        }

      val constructorClass = typeSymbol.declarations.find { sym =>
        sym.isClassDef && hasIdAnnotation(sym)
      }.orElse {
        typeSymbol.declarations.find { sym =>
          sym.isClassDef && sym.name == "Id"
        }
      }
      constructorClass match {
        case None =>
          report.errorAndAbort(
            s"Agent trait ${typeSymbol.name} must define a `class Id(...)` to declare its constructor parameters. Use `class Id()` for agents with no constructor parameters."
          )
        case Some(classSym) =>
          val primaryCtor = classSym.primaryConstructor
          primaryCtor.paramSymss.flatten.collect {
            case sym if sym.isTerm =>
              sym.tree match {
                case v: ValDef => (sym.name, v.tpt.tpe.show)
                case _         => (sym.name, "Any")
              }
          }
      }
    }

    // Build JSON
    val traitFqn    = typeSymbol.fullName
    val packageName = {
      val fqn     = traitFqn
      val lastDot = fqn.lastIndexOf('.')
      if (lastDot > 0) fqn.substring(0, lastDot) else ""
    }
    val simpleName = typeSymbol.name

    val sb = new StringBuilder
    sb.append("{")
    writeKey(sb, "traitFqn"); writeString(sb, traitFqn); sb.append(",")
    writeKey(sb, "packageName"); writeString(sb, packageName); sb.append(",")
    writeKey(sb, "simpleName"); writeString(sb, simpleName); sb.append(",")
    writeKey(sb, "typeName"); writeString(sb, rawTypeName); sb.append(",")
    writeKey(sb, "constructor"); writeConstructor(sb, constructorParams); sb.append(",")
    writeKey(sb, "metadata"); writeMetadata(sb, description, mode, snapshotting)
    sb.append("}")

    Expr(sb.toString)
  }

  private def extractStringArg(using
    Quotes
  )(
    symbol: quotes.reflect.Symbol,
    annFQN: String,
    argName: String,
    positionalIndex: Int
  ): Option[String] = {
    import quotes.reflect.*
    symbol.annotations.collectFirst {
      case Apply(Select(New(tpt), _), args) if tpt.tpe.dealias.typeSymbol.fullName == annFQN =>
        args.collectFirst {
          case NamedArg(`argName`, Literal(StringConstant(v))) if v.nonEmpty => v
        }.orElse {
          args.lift(positionalIndex).collect {
            case Literal(StringConstant(v)) if v.nonEmpty => v
          }
        }
    }.flatten
  }

  // ── JSON helpers (mirroring AgentSurfaceIRCodec format) ─────────────────

  private def writeKey(sb: StringBuilder, key: String): Unit = {
    writeString(sb, key); sb.append(":")
  }

  private def writeString(sb: StringBuilder, s: String): Unit = {
    sb.append('"')
    var i = 0
    while (i < s.length) {
      s.charAt(i) match {
        case '"'           => sb.append("\\\"")
        case '\\'          => sb.append("\\\\")
        case '\n'          => sb.append("\\n")
        case '\r'          => sb.append("\\r")
        case '\t'          => sb.append("\\t")
        case c if c < 0x20 =>
          sb.append("\\u")
          sb.append(String.format("%04x", Int.box(c.toInt)))
        case c => sb.append(c)
      }
      i += 1
    }
    sb.append('"')
  }

  private def writeConstructor(sb: StringBuilder, params: List[(String, String)]): Unit = {
    sb.append("{")
    writeKey(sb, "params")
    sb.append("[")
    var first = true
    params.foreach { case (name, typeExpr) =>
      if (!first) sb.append(",")
      first = false
      sb.append("{")
      writeKey(sb, "name"); writeString(sb, name); sb.append(",")
      writeKey(sb, "typeExpr"); writeString(sb, typeExpr)
      sb.append("}")
    }
    sb.append("]")
    sb.append("}")
  }

  private def writeMetadata(
    sb: StringBuilder,
    description: Option[String],
    mode: String,
    snapshotting: String
  ): Unit = {
    sb.append("{")
    writeKey(sb, "description")
    description match {
      case Some(d) => writeString(sb, d)
      case None    => sb.append("null")
    }
    sb.append(",")
    writeKey(sb, "mode"); writeString(sb, mode); sb.append(",")
    writeKey(sb, "snapshotting"); writeString(sb, snapshotting)
    sb.append("}")
  }
}
