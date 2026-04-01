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

import scala.reflect.macros.blackbox

/**
 * Macro that exports agent surface metadata as a JSON string literal (Scala 2).
 *
 * The JSON format matches [[golem.codegen.ir.AgentSurfaceIR]].
 */
object AgentSurfaceExportMacro {
  def exportJson[T]: String = macro AgentSurfaceExportMacroImpl.exportJsonImpl[T]
}

object AgentSurfaceExportMacroImpl {

  def exportJsonImpl[T: c.WeakTypeTag](c: blackbox.Context): c.Expr[String] = {
    import c.universe._

    val tpe        = weakTypeOf[T]
    val typeSymbol = tpe.typeSymbol

    if (!typeSymbol.isClass || !typeSymbol.asClass.isTrait)
      c.abort(c.enclosingPosition, s"AgentSurfaceExport target must be a trait, found: ${typeSymbol.fullName}")

    val agentDefinitionFQN        = "golem.runtime.annotations.agentDefinition"
    val descriptionAnnotationType = typeOf[golem.runtime.annotations.description]

    def isAgentDefinitionAnn(ann: Annotation): Boolean =
      ann.tree.tpe != null && ann.tree.tpe.typeSymbol.fullName == agentDefinitionFQN

    // Extract @agentDefinition
    val hasAnn = typeSymbol.annotations.exists(a => isAgentDefinitionAnn(a))
    if (!hasAnn)
      c.abort(c.enclosingPosition, s"Missing @agentDefinition(...) on agent trait: ${typeSymbol.fullName}")

    // Extract typeName
    val rawTypeName: String = typeSymbol.annotations.collectFirst {
      case ann if isAgentDefinitionAnn(ann) =>
        ann.tree.children.tail.collectFirst { case Literal(Constant(s: String)) => s }.getOrElse("")
    }.getOrElse("")

    val typeName: String = {
      val trimmed = rawTypeName.trim
      if (trimmed.nonEmpty) trimmed else typeSymbol.name.decodedName.toString
    }

    // Extract description
    val description: Option[String] = typeSymbol.annotations.collectFirst {
      case ann if ann.tree.tpe != null && ann.tree.tpe =:= descriptionAnnotationType =>
        ann.tree.children.tail.collectFirst { case Literal(Constant(s: String)) => s }
    }.flatten

    // Extract mode — agentDefinition(typeName, mode, ...)
    val mode: String = typeSymbol.annotations.collectFirst {
      case ann if isAgentDefinitionAnn(ann) =>
        val args = ann.tree.children.tail
        args.drop(1).headOption.flatMap {
          case Literal(Constant(s: String)) =>
            val v = s.trim.toLowerCase
            Some(v)
          case Literal(Constant(null)) =>
            None
          case Select(_, TermName(name)) =>
            Some(name.toLowerCase)
          case Ident(TermName(name)) =>
            Some(name.toLowerCase)
          case _ => None
        }
    }.flatten.getOrElse("durable")

    // Extract snapshotting — positional index 7 or named "snapshotting"
    val snapshotting: String = typeSymbol.annotations.collectFirst {
      case ann if isAgentDefinitionAnn(ann) =>
        val args = ann.tree.children.tail
        args.collectFirst { case NamedArg(Ident(TermName("snapshotting")), Literal(Constant(s: String))) =>
          s
        }.orElse {
          args.lift(7).collect { case Literal(Constant(s: String)) => s }
        }
    }.flatten.getOrElse("disabled")

    // Extract constructor params
    val constructorParams: List[(String, String)] = {
      val idAnnotationType = typeOf[golem.runtime.annotations.id]

      val annotatedClass = tpe.members.collectFirst {
        case sym
            if sym.isClass && !sym.isMethod &&
              sym.annotations.exists(ann => ann.tree.tpe != null && ann.tree.tpe =:= idAnnotationType) =>
          sym
      }

      val constructorClass = annotatedClass.orElse {
        val byName = tpe.member(TypeName("Id"))
        if (byName == NoSymbol) None else Some(byName)
      }.getOrElse {
        c.abort(
          c.enclosingPosition,
          s"Agent trait ${typeSymbol.fullName} must define a `class Id(...)` to declare its constructor parameters. Use `class Id()` for agents with no constructor parameters."
        )
      }
      val primaryCtor = constructorClass.asClass.primaryConstructor.asMethod
      primaryCtor.paramLists.flatten.filter(_.isTerm).map { param =>
        (param.name.toString, param.typeSignature.toString)
      }
    }

    // Build JSON
    val traitFqn    = typeSymbol.fullName
    val packageName = {
      val lastDot = traitFqn.lastIndexOf('.')
      if (lastDot > 0) traitFqn.substring(0, lastDot) else ""
    }
    val simpleName = typeSymbol.name.decodedName.toString

    val sb = new StringBuilder
    sb.append("{")
    writeKey(sb, "traitFqn"); writeString(sb, traitFqn); sb.append(",")
    writeKey(sb, "packageName"); writeString(sb, packageName); sb.append(",")
    writeKey(sb, "simpleName"); writeString(sb, simpleName); sb.append(",")
    writeKey(sb, "typeName"); writeString(sb, typeName); sb.append(",")
    writeKey(sb, "constructor"); writeConstructor(sb, constructorParams); sb.append(",")
    writeKey(sb, "metadata"); writeMetadata(sb, description, mode, snapshotting)
    sb.append("}")

    c.Expr[String](Literal(Constant(sb.toString)))
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
