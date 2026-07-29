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

package golem.runtime.macros

import golem.schema.{PathDirection, PathKind}
import golem.tool.*

import scala.quoted.*

/**
 * Compile-time parsing and classification shared by the tool macros: reads a
 * `@toolDefinition` trait into an IR, classifies every method parameter into
 * its command surface (global/option/flag/positional/tail/stream/principal),
 * infers the tail positional and its re-projection plan, and validates the
 * authoring surface with the same rules as the Rust SDK's `#[tool_definition]`
 * macro.
 */
private[macros] class ToolMacroCore(using val q: Quotes) {
  import q.reflect.*

  private val ToolDefinitionFQN = "golem.runtime.annotations.toolDefinition"
  private val CommandFQN        = "golem.runtime.annotations.command"
  private val AnnotationsFQN    = "golem.runtime.annotations.annotations"
  private val ArgFQN            = "golem.runtime.annotations.arg"
  private val ConstraintFQN     = "golem.runtime.annotations.constraint"
  private val ResultFQN         = "golem.runtime.annotations.result"
  private val ErrorFQN          = "golem.runtime.annotations.error"
  private val ExampleFQN        = "golem.runtime.annotations.example"
  private val ValueIsFQN        = "golem.runtime.annotations.ValueIs"
  private val ImpliesFQN        = "golem.runtime.annotations.Implies"
  private val ForbidsFQN        = "golem.runtime.annotations.Forbids"
  private val PrincipalFQN      = "golem.Principal"
  private val StdinFQN          = "golem.tool.ToolInputStream"
  private val StdoutFQN         = "golem.tool.ToolOutputStream"

  // -------------------------------------------------------------------------
  // Name conversion (port of the Rust SDK's to_kebab_case)
  // -------------------------------------------------------------------------

  def kebabCase(ident: String): String = {
    val out             = new StringBuilder
    val chars           = ident.toCharArray
    var i               = 0
    def pushSep(): Unit =
      if (out.nonEmpty && out.last != '-') out += '-'
    while (i < chars.length) {
      val c = chars(i)
      if (c == '_' || c == '-') pushSep()
      else if (c.isUpper) {
        val prev     = if (i > 0) Some(chars(i - 1)) else None
        val next     = if (i + 1 < chars.length) Some(chars(i + 1)) else None
        val boundary =
          prev.exists(p => p.isLower || p.isDigit) ||
            (prev.exists(_.isUpper) && next.exists(_.isLower))
        if (boundary) pushSep()
        out += c.toLower
      } else out += c
      i += 1
    }
    out.result()
  }

  // -------------------------------------------------------------------------
  // Annotation helpers
  // -------------------------------------------------------------------------

  private def stripTerm(t: Term): Term =
    t match {
      case Inlined(_, _, inner) => stripTerm(inner)
      case Typed(inner, _)      => stripTerm(inner)
      case NamedArg(_, inner)   => stripTerm(inner)
      case _                    => t
    }

  private def isDefaultArg(t: Term): Boolean = {
    val s = stripTerm(t)
    s.symbol != Symbol.noSymbol && s.symbol.name.contains("$default$")
  }

  /** Annotations of `sym` with the given class, in source order. */
  private def annotationsOf(sym: Symbol, fqn: String): List[Term] =
    sym.annotations.filter {
      case Apply(Select(New(tpt), _), _) => tpt.tpe.dealias.typeSymbol.fullName == fqn
      case _                             => false
    }.reverse

  /**
   * Extracts the explicitly authored arguments of an annotation (or helper
   * case-class application) as a name→term map, skipping compiler-inserted
   * default arguments.
   */
  private def namedArgs(args: List[Term], paramNames: List[String]): Map[String, Term] =
    args.zipWithIndex.flatMap {
      case (arg, _) if isDefaultArg(arg) => None
      case (NamedArg(name, value), _)    => Some(name -> value)
      case (value, idx)                  =>
        if (idx < paramNames.length) Some(paramNames(idx) -> value) else None
    }.toMap

  private def annotationValues(ann: Term, paramNames: List[String]): Map[String, Term] =
    ann match {
      case Apply(Select(New(_), _), args) => namedArgs(args, paramNames)
      case _                              => Map.empty
    }

  private def constString(t: Term, what: String, pos: Position): String =
    stripTerm(t) match {
      case Literal(StringConstant(s)) => s
      case other                      =>
        report.errorAndAbort(s"$what must be a string literal", pos)
    }

  private def constChar(t: Term, what: String, pos: Position): Char =
    stripTerm(t) match {
      case Literal(CharConstant(c)) => c
      case _                        => report.errorAndAbort(s"$what must be a character literal", pos)
    }

  private def constBoolean(t: Term, what: String, pos: Position): Boolean =
    stripTerm(t) match {
      case Literal(BooleanConstant(b)) => b
      case _                           => report.errorAndAbort(s"$what must be a boolean literal", pos)
    }

  private def constInt(t: Term, what: String, pos: Position): Int =
    stripTerm(t) match {
      case Literal(IntConstant(i)) => i
      case _                       => report.errorAndAbort(s"$what must be an integer literal", pos)
    }

  /** Elements of an `Array(...)` literal expression, if `t` is one. */
  private def arrayElems(t: Term): Option[List[Term]] = {
    def isArrayApply(fn: Term): Boolean =
      fn match {
        case Select(qual, "apply") =>
          val fullName = qual.tpe.typeSymbol.fullName
          fullName == "scala.Array$" || qual.symbol.fullName == "scala.Array$"
        case TypeApply(inner, _) => isArrayApply(inner)
        case _                   => false
      }
    def collect(args: List[Term]): List[Term] =
      args.flatMap { a =>
        stripTerm(a) match {
          case Repeated(elems, _) => elems.map(stripTerm)
          case other              =>
            other match {
              case Repeated(elems, _) => elems.map(stripTerm)
              case _                  => List(other)
            }
        }
      }
    stripTerm(t) match {
      case Apply(Apply(fn, realArgs), _) if isArrayApply(fn) => Some(collect(realArgs))
      case Apply(fn, realArgs) if isArrayApply(fn)           => Some(collect(realArgs))
      case _                                                 => None
    }
  }

  private def stringArray(t: Term, what: String, pos: Position): List[String] =
    arrayElems(t) match {
      case Some(elems) => elems.map(e => constString(e, s"each entry of $what", pos))
      case None        => report.errorAndAbort(s"$what must be an Array of string literals", pos)
    }

  /** The two components of a tuple literal (`(a, b)` or `a -> b`), if any. */
  private def tupleElems(t: Term): Option[(Term, Term)] =
    stripTerm(t) match {
      case Apply(TypeApply(Select(qual, "apply"), _), List(a, b))
          if qual.tpe.typeSymbol.fullName == "scala.Tuple2$" || qual.symbol.fullName == "scala.Tuple2$" =>
        Some((stripTerm(a), stripTerm(b)))
      case Apply(Select(qual, "apply"), List(a, b))
          if qual.tpe.typeSymbol.fullName == "scala.Tuple2$" || qual.symbol.fullName == "scala.Tuple2$" =>
        Some((stripTerm(a), stripTerm(b)))
      case Apply(TypeApply(Select(arrow, "->"), _), List(b)) =>
        stripTerm(arrow) match {
          case Apply(TypeApply(_, _), List(a)) => Some((stripTerm(a), stripTerm(b)))
          case Apply(_, List(a))               => Some((stripTerm(a), stripTerm(b)))
          case _                               => None
        }
      case _ => None
    }

  private val literalHint =
    "must be a literal value (string, number, bool, char, or an Array/tuple of literals)"

  /** Interprets a metadata literal expression into a [[ToolLiteral]]. */
  def toolLiteral(t0: Term, what: String, pos: Position): ToolLiteral = {
    val t = stripTerm(t0)
    t match {
      case Literal(BooleanConstant(b))          => ToolLiteral.BoolLiteral(b)
      case Literal(StringConstant(s))           => ToolLiteral.StrLiteral(s)
      case Literal(CharConstant(c))             => ToolLiteral.CharLiteral(c.toInt)
      case Literal(ByteConstant(v))             => ToolLiteral.IntLiteral(BigInt(v.toInt))
      case Literal(ShortConstant(v))            => ToolLiteral.IntLiteral(BigInt(v.toInt))
      case Literal(IntConstant(v))              => ToolLiteral.IntLiteral(BigInt(v))
      case Literal(LongConstant(v))             => ToolLiteral.IntLiteral(BigInt(v))
      case Literal(FloatConstant(v))            => ToolLiteral.FloatLiteral(v.toDouble)
      case Literal(DoubleConstant(v))           => ToolLiteral.FloatLiteral(v)
      case Select(inner, "unary_-")             => negateLiteral(toolLiteral(inner, what, pos), pos)
      case Apply(Select(inner, "unary_-"), Nil) =>
        negateLiteral(toolLiteral(inner, what, pos), pos)
      case _ =>
        arrayElems(t) match {
          case Some(elems) =>
            val tuples = elems.map(tupleElems)
            if (elems.nonEmpty && tuples.forall(_.isDefined))
              ToolLiteral.MapLiteral(tuples.map(_.get).map { case (k, v) =>
                (toolLiteral(k, what, pos), toolLiteral(v, what, pos))
              })
            else ToolLiteral.ListLiteral(elems.map(toolLiteral(_, what, pos)))
          case None =>
            report.errorAndAbort(s"$what $literalHint", pos)
        }
    }
  }

  private def negateLiteral(lit: ToolLiteral, pos: Position): ToolLiteral =
    lit match {
      case ToolLiteral.IntLiteral(v)   => ToolLiteral.IntLiteral(-v)
      case ToolLiteral.FloatLiteral(v) => ToolLiteral.FloatLiteral(-v)
      case _                           => report.errorAndAbort("unsupported negated literal", pos)
    }

  /**
   * A `min`/`max`/`bounds` component: a numeric literal, or a string literal
   * holding a decimal integer (the escape hatch for bounds beyond `Long`, e.g.
   * the u64 maximum).
   */
  private def boundLiteral(t: Term, what: String, pos: Position): ToolLiteral =
    stripTerm(t) match {
      case Literal(StringConstant(s)) =>
        try ToolLiteral.IntLiteral(BigInt(s.trim))
        catch {
          case _: NumberFormatException =>
            report.errorAndAbort(s"$what must be a numeric literal or a decimal integer string", pos)
        }
      case _ => toolLiteral(t, what, pos)
    }

  // -------------------------------------------------------------------------
  // Doc extraction
  // -------------------------------------------------------------------------

  private def examplesOf(sym: Symbol): List[Example] =
    annotationsOf(sym, ExampleFQN).map { ann =>
      val values = annotationValues(ann, List("body", "title"))
      val pos    = ann.pos
      val body   = values.get("body") match {
        case Some(t) => constString(t, "body", pos)
        case None    => report.errorAndAbort("@example is missing `body`", pos)
      }
      val title = values.get("title").map(constString(_, "title", pos)).getOrElse("")
      Example(title, body)
    }

  /** Doc from a symbol's Scaladoc plus its `@example` annotations. */
  def docOf(sym: Symbol): Doc = {
    val (summary, description) =
      sym.docstring.flatMap(Scaladoc.clean) match {
        case Some(cleaned) => Scaladoc.summaryAndDescription(cleaned)
        case None          => ("", "")
      }
    Doc(summary, description, examplesOf(sym))
  }

  def argDoc(text: Option[String]): Doc = Doc(text.getOrElse(""), "")

  // -------------------------------------------------------------------------
  // @arg IR
  // -------------------------------------------------------------------------

  final case class ArgIR(
    key: String,
    pos: Position,
    scope: Option[String],
    argKind: Option[String], // "flag" | "count-flag"
    pathKind: Option[PathKind],
    short: Option[Char],
    aliases: List[String],
    env: Option[String],
    required: Option[Boolean],
    negatable: Option[Boolean],
    optionalScalar: Boolean,
    repeatable: Option[String],
    delim: Option[Char],
    default: Option[ToolLiteral],
    defaultIsBool: Boolean,
    separator: Option[String],
    verbatim: Boolean,
    acceptsStdio: Boolean,
    regex: Option[String],
    minLength: Option[Int],
    maxLength: Option[Int],
    direction: Option[PathDirection],
    mime: Option[List[String]],
    schemes: Option[List[String]],
    min: Option[ToolLiteral],
    max: Option[ToolLiteral],
    unit: Option[String],
    doc: Option[String],
    valueName: Option[String]
  ) {
    def hasTextRefinement: Boolean = regex.isDefined || minLength.isDefined || maxLength.isDefined
    def hasPathRefinement: Boolean = pathKind.isDefined || direction.isDefined || mime.isDefined
    def hasUrlRefinement: Boolean  = schemes.isDefined
    def hasMinOrMax: Boolean       = min.isDefined || max.isDefined

    def refinements(includeMinMax: Boolean): ToolArgRefinements =
      ToolArgRefinements(
        regex = regex,
        minLength = minLength,
        maxLength = maxLength,
        pathKind = pathKind,
        direction = direction,
        mime = mime,
        schemes = schemes,
        min = if (includeMinMax) min else None,
        max = if (includeMinMax) max else None,
        unit = unit
      )
  }

  private val argParamNames = List(
    "name",
    "scope",
    "kind",
    "short",
    "aliases",
    "env",
    "required",
    "negatable",
    "optionalScalar",
    "repeatable",
    "delim",
    "default",
    "separator",
    "verbatim",
    "acceptsStdio",
    "regex",
    "minLength",
    "maxLength",
    "direction",
    "mime",
    "schemes",
    "min",
    "max",
    "bounds",
    "unit",
    "doc",
    "valueName"
  )

  private def parseArg(ann: Term): ArgIR = {
    val pos    = ann.pos
    val values = annotationValues(ann, argParamNames)

    val key = values.get("name") match {
      case Some(t) => constString(t, "name", pos)
      case None    =>
        report.errorAndAbort(
          "@arg(...) must start with a parameter name, e.g. `@arg(\"input\", scope = \"positional\")`",
          pos
        )
    }

    val scope = values.get("scope").map(constString(_, "scope", pos)).map { s =>
      s match {
        case "global" | "positional" | "option" | "flag" | "tail" => s
        case other                                                =>
          report.errorAndAbort(
            s"invalid arg scope `$other`; expected one of: global, positional, option, flag, tail",
            pos
          )
      }
    }

    var argKind: Option[String]    = None
    var pathKind: Option[PathKind] = None
    values.get("kind").map(constString(_, "kind", pos)).foreach {
      case k @ ("flag" | "count-flag") => argKind = Some(k)
      case "file"                      => pathKind = Some(PathKind.File)
      case "dir" | "directory"         => pathKind = Some(PathKind.Directory)
      case "any"                       => pathKind = Some(PathKind.Any)
      case other                       =>
        report.errorAndAbort(
          s"invalid kind `$other`; expected one of: flag, count-flag (arg kind) or file, dir, any (path kind)",
          pos
        )
    }

    val repeatable = values.get("repeatable").map(constString(_, "repeatable", pos)).map {
      case m @ ("repeated" | "delimited" | "either") => m
      case other                                     =>
        report.errorAndAbort(s"invalid repeatable mode `$other`; expected: repeated, delimited, either", pos)
    }

    val direction = values.get("direction").map(constString(_, "direction", pos)).map {
      case "input" | "in"                => PathDirection.Input
      case "output" | "out"              => PathDirection.Output
      case "inout" | "in-out" | "in_out" => PathDirection.InOut
      case other                         =>
        report.errorAndAbort(s"invalid direction `$other`; expected: input, output, inout", pos)
    }

    val bounds = values.get("bounds").map { t =>
      tupleElems(t) match {
        case Some((a, b)) =>
          (boundLiteral(a, "bounds", pos), boundLiteral(b, "bounds", pos))
        case None =>
          report.errorAndAbort("bounds must be a 2-tuple `(min, max)`", pos)
      }
    }
    val rawMin = values.get("min").map(boundLiteral(_, "min", pos))
    val rawMax = values.get("max").map(boundLiteral(_, "max", pos))
    if (bounds.isDefined && (rawMin.isDefined || rawMax.isDefined))
      report.errorAndAbort("use either `bounds = (min, max)` or `min`/`max`, not both", pos)

    val defaultTerm = values.get("default")
    val default     = defaultTerm.map(toolLiteral(_, "default", pos))

    ArgIR(
      key = key,
      pos = pos,
      scope = scope,
      argKind = argKind,
      pathKind = pathKind,
      short = values.get("short").map(constChar(_, "short", pos)),
      aliases = values.get("aliases").map(stringArray(_, "aliases", pos)).getOrElse(Nil),
      env = values.get("env").map(constString(_, "env", pos)),
      required = values.get("required").map(constBoolean(_, "required", pos)),
      negatable = values.get("negatable").map(constBoolean(_, "negatable", pos)),
      optionalScalar = values.get("optionalScalar").exists(constBoolean(_, "optionalScalar", pos)),
      repeatable = repeatable,
      delim = values.get("delim").map(constChar(_, "delim", pos)),
      default = default,
      defaultIsBool = default.exists(_.isInstanceOf[ToolLiteral.BoolLiteral]),
      separator = values.get("separator").map(constString(_, "separator", pos)),
      verbatim = values.get("verbatim").exists(constBoolean(_, "verbatim", pos)),
      acceptsStdio = values.get("acceptsStdio").exists(constBoolean(_, "acceptsStdio", pos)),
      regex = values.get("regex").map(constString(_, "regex", pos)),
      minLength = values.get("minLength").map(constInt(_, "minLength", pos)),
      maxLength = values.get("maxLength").map(constInt(_, "maxLength", pos)),
      direction = direction,
      mime = values.get("mime").map(stringArray(_, "mime", pos)),
      schemes = values.get("schemes").map(stringArray(_, "schemes", pos)),
      min = rawMin.orElse(bounds.map(_._1)),
      max = rawMax.orElse(bounds.map(_._2)),
      unit = values.get("unit").map(constString(_, "unit", pos)),
      doc = values.get("doc").map(constString(_, "doc", pos)),
      valueName = values.get("valueName").map(constString(_, "valueName", pos))
    )
  }

  // -------------------------------------------------------------------------
  // Constraints
  // -------------------------------------------------------------------------

  private def parseRef(t0: Term, pos: Position): ExtendedRef = {
    val t = stripTerm(t0)
    t match {
      case Literal(StringConstant(s))                 => ExtendedRef.Present(s)
      case Apply(fn, args) if applyOf(fn, ValueIsFQN) =>
        val values = namedArgs(args, List("name", "value"))
        val name   = values.get("name") match {
          case Some(n) => constString(n, "ValueIs name", pos)
          case None    => report.errorAndAbort("ValueIs(...) is missing `name`", pos)
        }
        val value = values.get("value") match {
          case Some(v) => toolLiteral(v, "ValueIs value", pos)
          case None    => report.errorAndAbort("ValueIs(...) is missing `value`", pos)
        }
        ExtendedRef.ValueIs(ExtendedValueIsRef(name, ExtendedValueIsLiteral.Deferred(value)))
      case _ =>
        report.errorAndAbort(
          "expected an argument name string or `ValueIs(name, value)`",
          pos
        )
    }
  }

  private def applyOf(fn: Term, companionFqn: String): Boolean =
    fn match {
      case Select(qual, "apply") =>
        qual.tpe.typeSymbol.fullName == companionFqn + "$" ||
        qual.symbol.fullName == companionFqn + "$" ||
        qual.tpe.typeSymbol.fullName == companionFqn
      case TypeApply(inner, _) => applyOf(inner, companionFqn)
      case _                   => false
    }

  private def parseRefs(t: Term, pos: Position): List[ExtendedRef] =
    arrayElems(t) match {
      case Some(elems) => elems.map(parseRef(_, pos))
      case None        => List(parseRef(t, pos))
    }

  private def parseQuantifier(t: Term, pos: Position): Quantifier =
    constString(t, "quantifier", pos) match {
      case "all" => Quantifier.All
      case "any" => Quantifier.Any
      case other =>
        report.errorAndAbort(s"invalid quantifier `$other`; expected `all` or `any`", pos)
    }

  private def parseConstraint(ann: Term): ExtendedConstraint = {
    val pos    = ann.pos
    val values = annotationValues(
      ann,
      List("requiresAll", "requiresAny", "allOrNone", "mutexGroups", "implies", "forbids")
    )
    if (values.size != 1)
      report.errorAndAbort(
        "@constraint(...) must contain exactly one constraint, e.g. `allOrNone = Array(...)` or `implies = Implies(...)`",
        pos
      )
    val (key, term) = values.head
    key match {
      case "requiresAll" => ExtendedConstraint.RequiresAll(parseRefs(term, pos))
      case "requiresAny" => ExtendedConstraint.RequiresAny(parseRefs(term, pos))
      case "allOrNone"   => ExtendedConstraint.AllOrNone(parseRefs(term, pos))
      case "mutexGroups" =>
        arrayElems(term) match {
          case Some(groups) =>
            val parsed = groups.map { g =>
              arrayElems(g) match {
                case Some(refs) => ExtendedRefGroup(refs.map(parseRef(_, pos)))
                case None       =>
                  report.errorAndAbort(
                    "mutexGroups must be an Array of groups, e.g. `Array(Array(\"add\"), Array(\"delete\"))`; " +
                      "each group must itself be an Array of argument refs",
                    pos
                  )
              }
            }
            ExtendedConstraint.MutexGroups(parsed)
          case None =>
            report.errorAndAbort(
              "mutexGroups must be an Array of groups, e.g. `Array(Array(\"add\"), Array(\"delete\"))`",
              pos
            )
        }
      case "implies" =>
        stripTerm(term) match {
          case Apply(fn, args) if applyOf(fn, ImpliesFQN) =>
            val v   = namedArgs(args, List("lhs", "rhs", "lhsQuant", "rhsQuant"))
            val lhs = v.get("lhs") match {
              case Some(t) => parseRefs(t, pos)
              case None    => report.errorAndAbort("Implies(...) is missing `lhs`", pos)
            }
            val rhs = v.get("rhs") match {
              case Some(t) => parseRefs(t, pos)
              case None    => report.errorAndAbort("Implies(...) is missing `rhs`", pos)
            }
            val lhsQuant = v.get("lhsQuant").map(parseQuantifier(_, pos)).getOrElse(Quantifier.All)
            val rhsQuant = v.get("rhsQuant").map(parseQuantifier(_, pos)).getOrElse(Quantifier.All)
            ExtendedConstraint.Implies(ExtendedImpliesC(lhsQuant, lhs, rhsQuant, rhs))
          case _ =>
            report.errorAndAbort("`implies` must be an `Implies(lhs = ..., rhs = ...)` value", pos)
        }
      case "forbids" =>
        stripTerm(term) match {
          case Apply(fn, args) if applyOf(fn, ForbidsFQN) =>
            val v   = namedArgs(args, List("lhs", "rhs", "lhsQuant"))
            val lhs = v.get("lhs") match {
              case Some(t) => parseRefs(t, pos)
              case None    => report.errorAndAbort("Forbids(...) is missing `lhs`", pos)
            }
            val rhs = v.get("rhs") match {
              case Some(t) => parseRefs(t, pos)
              case None    => report.errorAndAbort("Forbids(...) is missing `rhs`", pos)
            }
            val lhsQuant = v.get("lhsQuant").map(parseQuantifier(_, pos)).getOrElse(Quantifier.All)
            ExtendedConstraint.Forbids(ExtendedForbidsC(lhsQuant, lhs, rhs))
          case _ =>
            report.errorAndAbort("`forbids` must be a `Forbids(lhs = ..., rhs = ...)` value", pos)
        }
      case other =>
        report.errorAndAbort(s"unknown @constraint key `$other`", pos)
    }
  }

  // -------------------------------------------------------------------------
  // Type helpers
  // -------------------------------------------------------------------------

  def isPrincipal(tpe: TypeRepr): Boolean =
    tpe.dealias.typeSymbol.fullName == PrincipalFQN

  def isStdin(tpe: TypeRepr): Boolean =
    tpe.dealias.typeSymbol.fullName == StdinFQN

  def isStdout(tpe: TypeRepr): Boolean =
    tpe.dealias.typeSymbol.fullName == StdoutFQN

  def optionArg(tpe: TypeRepr): Option[TypeRepr] =
    tpe.dealias match {
      case AppliedType(base, List(arg)) if base.typeSymbol.fullName == "scala.Option" => Some(arg)
      case _                                                                          => None
    }

  private lazy val seqSym = Symbol.requiredClass("scala.collection.immutable.Seq")
  private lazy val mapSym = Symbol.requiredClass("scala.collection.immutable.Map")

  def seqItem(tpe: TypeRepr): Option[TypeRepr] = {
    val d = tpe.dealias
    if (d.derivesFrom(seqSym))
      d.baseType(seqSym) match {
        case AppliedType(_, List(item)) => Some(item)
        case _                          => None
      }
    else None
  }

  def isMapType(tpe: TypeRepr): Boolean =
    tpe.dealias.derivesFrom(mapSym)

  def isBool(tpe: TypeRepr): Boolean = tpe.dealias =:= TypeRepr.of[Boolean]
  def isInt(tpe: TypeRepr): Boolean  = tpe.dealias =:= TypeRepr.of[Int]
  def isUnit(tpe: TypeRepr): Boolean = tpe.dealias =:= TypeRepr.of[Unit]

  def futureArg(tpe: TypeRepr): Option[TypeRepr] =
    tpe.dealias match {
      case AppliedType(base, List(arg)) if base.typeSymbol.fullName == "scala.concurrent.Future" =>
        Some(arg)
      case _ => None
    }

  def eitherArgs(tpe: TypeRepr): Option[(TypeRepr, TypeRepr)] =
    tpe.dealias match {
      case AppliedType(base, List(l, r)) if base.typeSymbol.fullName == "scala.util.Either" =>
        Some((l, r))
      case _ => None
    }

  def isToolDefinitionTrait(tpe: TypeRepr): Boolean = {
    val sym = tpe.dealias.typeSymbol
    sym.flags.is(Flags.Trait) && annotationsOf(sym, ToolDefinitionFQN).nonEmpty
  }

  // -------------------------------------------------------------------------
  // Return shape
  // -------------------------------------------------------------------------

  sealed trait ReturnKind
  object ReturnKind {
    case object UnitK                                             extends ReturnKind
    final case class Value(tpe: TypeRepr)                         extends ReturnKind
    final case class EitherK(err: TypeRepr, ok: Option[TypeRepr]) extends ReturnKind
  }

  final case class ReturnShape(async: Boolean, kind: ReturnKind, raw: TypeRepr)

  def returnShape(tpe: TypeRepr): ReturnShape = {
    val (async, inner) = futureArg(tpe) match {
      case Some(t) => (true, t)
      case None    => (false, tpe)
    }
    val kind = eitherArgs(inner) match {
      case Some((err, ok)) =>
        ReturnKind.EitherK(err, if (isUnit(ok)) None else Some(ok))
      case None =>
        if (isUnit(inner)) ReturnKind.UnitK else ReturnKind.Value(inner)
    }
    ReturnShape(async, kind, inner)
  }

  // -------------------------------------------------------------------------
  // Trait / method IR
  // -------------------------------------------------------------------------

  final case class ParamIR(
    sym: Symbol,
    name: String,
    kebab: String,
    tpe: TypeRepr,
    arg: Option[ArgIR]
  )

  final case class MethodIR(
    sym: Symbol,
    methodName: String,
    commandName: String,
    nameOverride: Option[String],
    aliases: List[String],
    doc: Doc,
    annotations: Option[CommandAnnotations],
    params: List[ParamIR],
    constraints: List[ExtendedConstraint],
    resultAttr: Option[(List[String], Option[String])],
    shape: ReturnShape,
    subtreeTrait: Option[TypeRepr],
    isRoot: Boolean
  )

  final case class ToolIR(
    traitSym: Symbol,
    identity: String,
    toolName: String,
    version: String,
    traitDoc: Doc,
    rootMethod: Option[MethodIR],
    childMethods: List[MethodIR]
  )

  def parseTool(traitRepr: TypeRepr): ToolIR = {
    val traitSym = traitRepr.typeSymbol
    if (!traitSym.flags.is(Flags.Trait))
      report.errorAndAbort(s"@toolDefinition target must be a trait, found: ${traitSym.fullName}")

    val toolDefAnns = annotationsOf(traitSym, ToolDefinitionFQN)
    if (toolDefAnns.isEmpty)
      report.errorAndAbort(s"missing @toolDefinition(...) on tool trait: ${traitSym.fullName}")
    val toolDefValues = annotationValues(toolDefAnns.head, List("name", "version"))
    val explicitName  =
      toolDefValues.get("name").map(constString(_, "name", toolDefAnns.head.pos)).filter(_.nonEmpty)
    val version =
      toolDefValues.get("version").map(constString(_, "version", toolDefAnns.head.pos)).getOrElse("0.0.0")
    val toolName = explicitName.getOrElse(kebabCase(traitSym.name))

    val methodSyms = traitSym.declarations.filter { d =>
      d.isDefDef && d.flags.is(Flags.Deferred) && !d.isClassConstructor
    }

    val methods = methodSyms.map(parseMethod(_, toolName))

    val rootCandidates = methods.filter(_.isRoot)
    if (rootCandidates.length > 1)
      report.errorAndAbort(
        s"multiple methods map to the tool's root command name `$toolName`; only one method may " +
          "be the implicit-body handler (§5.8.1)",
        traitSym.pos.getOrElse(Position.ofMacroExpansion)
      )

    rootCandidates.headOption.foreach { root =>
      if (root.subtreeTrait.isDefined)
        report.errorAndAbort(
          "the implicit-body method cannot also be a subtree method",
          root.sym.pos.getOrElse(Position.ofMacroExpansion)
        )
      root.nameOverride.foreach { name =>
        if (name != toolName)
          report.errorAndAbort(
            s"the implicit-body method's @command(name = ${quoteStr(name)}) diverges from the tool " +
              s"name ${quoteStr(toolName)}; the root command name must equal the tool name (§5.8.1)",
            root.sym.pos.getOrElse(Position.ofMacroExpansion)
          )
      }
    }

    val children = methods.filterNot(_.isRoot)
    children.foreach { m =>
      if (m.commandName == toolName)
        report.errorAndAbort(
          s"command `${m.commandName}` collides with the tool's root command name; rename the " +
            "method or use @command(name = ...)",
          m.sym.pos.getOrElse(Position.ofMacroExpansion)
        )
    }

    ToolIR(
      traitSym = traitSym,
      identity = traitSym.fullName,
      toolName = toolName,
      version = version,
      traitDoc = docOf(traitSym),
      rootMethod = rootCandidates.headOption,
      childMethods = children
    )
  }

  private def quoteStr(s: String): String = "\"" + s + "\""

  private def parseMethod(sym: Symbol, toolName: String): MethodIR = {
    val pos = sym.pos.getOrElse(Position.ofMacroExpansion)

    val defdef = sym.tree match {
      case d: DefDef => d
      case other     => report.errorAndAbort(s"unable to read tool method ${sym.name}: $other", pos)
    }

    if (sym.paramSymss.exists(_.exists(_.isType)))
      report.errorAndAbort("tool methods must not have type parameters", pos)
    val termParamLists = sym.paramSymss.filter(_.forall(_.isTerm))
    if (termParamLists.length > 1)
      report.errorAndAbort("tool methods must have a single parameter list", pos)

    val paramSyms = termParamLists.headOption.getOrElse(Nil)

    // @command
    val commandAnns  = annotationsOf(sym, CommandFQN)
    var nameOverride = Option.empty[String]
    var aliases      = List.empty[String]
    commandAnns.foreach { ann =>
      val values = annotationValues(ann, List("name", "aliases"))
      values.get("name").map(constString(_, "name", ann.pos)).filter(_.nonEmpty).foreach { n =>
        nameOverride = Some(n)
      }
      values.get("aliases").foreach(t => aliases = stringArray(t, "aliases", ann.pos))
    }

    // @annotations
    val annAnns     = annotationsOf(sym, AnnotationsFQN)
    val annotations = annAnns.headOption.map { ann =>
      val values = annotationValues(ann, List("readOnly", "destructive", "idempotent", "openWorld"))
      CommandAnnotations(
        readOnly = values.get("readOnly").map(constBoolean(_, "readOnly", ann.pos)).getOrElse(false),
        destructive = values.get("destructive").map(constBoolean(_, "destructive", ann.pos)).getOrElse(true),
        idempotent = values.get("idempotent").map(constBoolean(_, "idempotent", ann.pos)).getOrElse(false),
        openWorld = values.get("openWorld").map(constBoolean(_, "openWorld", ann.pos)).getOrElse(true)
      )
    }

    // @result
    val resultAnns = annotationsOf(sym, ResultFQN)
    if (resultAnns.length > 1)
      report.errorAndAbort("a command may have at most one @result(...) annotation", pos)
    val resultAttr = resultAnns.headOption.map { ann =>
      val values     = annotationValues(ann, List("formatters", "default"))
      val formatters = values.get("formatters").map(stringArray(_, "formatters", ann.pos)).getOrElse(Nil)
      val default    = values.get("default").map(constString(_, "default", ann.pos)).filter(_.nonEmpty)
      (formatters, default)
    }

    // @constraint
    val constraints = annotationsOf(sym, ConstraintFQN).map(parseConstraint)

    // @arg
    val argIRs = annotationsOf(sym, ArgFQN).map(parseArg)

    val params = paramSyms.map { p =>
      val ptpe = p.tree match {
        case v: ValDef => v.tpt.tpe
        case other     => report.errorAndAbort(s"unsupported parameter declaration in ${sym.name}: $other", pos)
      }
      ParamIR(p, p.name, kebabCase(p.name), ptpe, None)
    }

    // Bind @arg entries to parameters by surface (kebab) or exact name.
    val bound = scala.collection.mutable.Map.empty[String, ArgIR]
    argIRs.foreach { a =>
      val param = params.find(p => p.kebab == a.key || p.name == a.key)
      param match {
        case None =>
          report.errorAndAbort(
            s"@arg(...) refers to unknown parameter `${a.key}`; the method has no such parameter",
            a.pos
          )
        case Some(p) =>
          if (bound.contains(p.name))
            report.errorAndAbort(s"duplicate @arg(...) for parameter `${a.key}`", a.pos)
          bound.update(p.name, a)
      }
    }
    val paramsWithArgs = params.map(p => p.copy(arg = bound.get(p.name)))

    val shape = returnShape(defdef.returnTpt.tpe)

    val subtreeTrait =
      if (isToolDefinitionTrait(shape.raw)) Some(shape.raw.dealias) else None

    val isRoot      = kebabCase(sym.name) == toolName
    val commandName = if (isRoot) toolName else nameOverride.getOrElse(kebabCase(sym.name))

    MethodIR(
      sym = sym,
      methodName = sym.name,
      commandName = commandName,
      nameOverride = nameOverride,
      aliases = aliases,
      doc = docOf(sym),
      annotations = annotations,
      params = paramsWithArgs,
      constraints = constraints,
      resultAttr = resultAttr,
      shape = shape,
      subtreeTrait = subtreeTrait,
      isRoot = isRoot
    )
  }

  // -------------------------------------------------------------------------
  // Classification
  // -------------------------------------------------------------------------

  sealed trait ShapeIR
  object ShapeIR {
    final case class Scalar(tpe: TypeRepr, optionalScalar: Boolean) extends ShapeIR
    final case class RList(item: TypeRepr, repetition: Repetition)  extends ShapeIR
    final case class RMap(mapTpe: TypeRepr, repetition: Repetition) extends ShapeIR
  }

  final case class OptionIR(
    long: String,
    short: Option[Char],
    aliases: List[String],
    doc: Doc,
    valueName: Option[String],
    shape: ShapeIR,
    refinements: ToolArgRefinements,
    default: Option[ToolLiteral],
    required: Boolean,
    env: Option[String]
  )

  final case class PositionalIR(
    name: String,
    doc: Doc,
    valueName: Option[String],
    tpe: TypeRepr,
    refinements: ToolArgRefinements,
    default: Option[ToolLiteral],
    required: Boolean,
    acceptsStdio: Boolean
  )

  final case class TailIR(
    name: String,
    doc: Doc,
    valueName: Option[String],
    item: TypeRepr,
    refinements: ToolArgRefinements,
    min: Int,
    max: Option[Int],
    separator: Option[String],
    verbatim: Boolean,
    acceptsStdio: Boolean
  )

  sealed trait PlanIR
  object PlanIR {
    final case class Plain(name: String) extends PlanIR
    final case class Vec(
      name: String,
      explicitTail: Boolean,
      optionalVec: Boolean,
      hasMinOrMaxAttr: Boolean,
      authoredTailSurrogate: Option[TailIR],
      laterOptionNames: List[String]
    ) extends PlanIR
  }

  /** How one parameter is supplied at invocation time (impl-macro view). */
  sealed trait ParamBindingIR
  object ParamBindingIR {
    final case class Field(canonicalName: String, tpe: TypeRepr, countFlag: Boolean = false) extends ParamBindingIR
    case object PrincipalB                                                                   extends ParamBindingIR
    case object StdinB                                                                       extends ParamBindingIR
    case object StdoutB                                                                      extends ParamBindingIR
  }

  final case class ClassifiedCommand(
    method: MethodIR,
    globalOptions: List[OptionIR],
    globalFlags: List[FlagSpec],
    fixed: List[PositionalIR],
    tail: Option[TailIR],
    bodyOptions: List[OptionIR],
    bodyFlags: List[FlagSpec],
    stdin: Option[StreamSpec],
    stdout: Option[StreamSpec],
    plan: List[PlanIR],
    bindings: List[ParamBindingIR]
  )

  /**
   * The surface names of the root command's globals, computed from the root
   * method's `scope = "global"` parameters. Used for inherited-redeclaration
   * detection when classifying non-root commands of the same trait.
   */
  def rootGlobalSurfacesOf(ir: ToolIR): List[(String, List[String])] =
    ir.rootMethod match {
      case None       => Nil
      case Some(root) =>
        root.params.flatMap { p =>
          if (p.arg.exists(_.scope.contains("global"))) {
            Some((p.kebab, p.arg.map(_.aliases).getOrElse(Nil)))
          } else None
        }
    }

  private def surfaceIntersects(
    names: List[String],
    global: (String, List[String])
  ): Boolean = {
    val globalNames = global._1 :: global._2
    names.exists(globalNames.contains)
  }

  /**
   * Classifies one leaf/root command method's parameters into their command
   * surfaces, mirroring the Rust `classify` decision order.
   */
  def classifyCommand(
    ir: ToolIR,
    m: MethodIR,
    rootGlobals: List[(String, List[String])]
  ): ClassifiedCommand = {
    val pos = m.sym.pos.getOrElse(Position.ofMacroExpansion)

    def perr(msg: String, p: ParamIR): Nothing =
      report.errorAndAbort(msg, p.arg.map(_.pos).getOrElse(pos))

    var globalOptions         = List.empty[OptionIR]
    var globalFlags           = List.empty[FlagSpec]
    var fixed                 = List.empty[PositionalIR]
    var tail                  = Option.empty[TailIR]
    var bodyOptions           = List.empty[OptionIR]
    var bodyFlags             = List.empty[FlagSpec]
    var stdin                 = Option.empty[StreamSpec]
    var stdout                = Option.empty[StreamSpec]
    var plan                  = List.empty[PlanIR]
    var bindings              = List.empty[ParamBindingIR]
    var sawOptionalPositional = false

    // ---- tail inference indices ------------------------------------------
    def positionalEligible(p: ParamIR): Boolean =
      if (isPrincipal(p.tpe) || isStdin(p.tpe) || isStdout(p.tpe)) false
      else {
        val a     = p.arg
        val scope = a.flatMap(_.scope)
        scope match {
          case Some("global") | Some("option") | Some("flag") => false
          case Some("positional") | Some("tail")              => true
          case _                                              =>
            if (a.flatMap(_.argKind).isDefined) false
            else {
              val base = optionArg(p.tpe).getOrElse(p.tpe)
              !isBool(base) && !isMapType(base)
            }
        }
      }

    def isInheritedRedecl(p: ParamIR): Boolean =
      if (m.isRoot) false
      else {
        val names = p.kebab :: p.arg.map(_.aliases).getOrElse(Nil)
        rootGlobals.exists(g => surfaceIntersects(names, g))
      }

    def vecTailRepresentable(p: ParamIR): Boolean =
      optionArg(p.tpe).isEmpty && seqItem(p.tpe).isDefined

    val eligibleIdx         = m.params.zipWithIndex.filter { case (p, _) => positionalEligible(p) }
    val lastValueIdx        = eligibleIdx.lastOption.map(_._2).getOrElse(-1)
    val lastNonInheritedIdx =
      eligibleIdx.filterNot { case (p, _) => isInheritedRedecl(p) }.lastOption.map(_._2).getOrElse(-1)

    def emitAsTail(p: ParamIR, idx: Int): Boolean =
      idx == lastValueIdx ||
        (idx == lastNonInheritedIdx && lastNonInheritedIdx != lastValueIdx &&
          p.arg.flatMap(_.scope).isEmpty && vecTailRepresentable(p))

    // ---- refinement family checks ----------------------------------------
    def rejectRefinements(a: ArgIR, context: String): Unit = {
      if (a.hasTextRefinement)
        report.errorAndAbort(
          s"text refinements (`regex`/`minLength`/`maxLength`) are not valid on $context",
          a.pos
        )
      if (a.hasPathRefinement)
        report.errorAndAbort(
          s"path refinements (`kind`/`direction`/`mime`) are not valid on $context",
          a.pos
        )
      if (a.hasUrlRefinement)
        report.errorAndAbort(s"url refinements (`schemes`) are not valid on $context", a.pos)
    }

    // Structural-attribute validity per surface: reject every authored key the
    // resolved surface does not consume.
    def rejectStructural(a: ArgIR, surface: String, allowed: Set[String]): Unit = {
      def bad(field: String): Nothing =
        report.errorAndAbort(s"`$field` is not valid on $surface", a.pos)
      if (a.short.isDefined && !allowed("short")) bad("short")
      if (a.aliases.nonEmpty && !allowed("aliases")) bad("aliases")
      if (a.env.isDefined && !allowed("env")) bad("env")
      if (a.required.isDefined && !allowed("required")) bad("required")
      if (a.negatable.isDefined && !allowed("negatable")) bad("negatable")
      if (a.optionalScalar && !allowed("optionalScalar")) bad("optionalScalar")
      if (a.repeatable.isDefined && !allowed("repeatable")) bad("repeatable")
      if (a.delim.isDefined && !allowed("delim")) bad("delim")
      if (a.default.isDefined && !allowed("default")) bad("default")
      if (a.separator.isDefined && !allowed("separator")) bad("separator")
      if (a.verbatim && !allowed("verbatim")) bad("verbatim")
      if (a.acceptsStdio && !allowed("acceptsStdio")) bad("acceptsStdio")
      if (a.valueName.isDefined && !allowed("valueName")) bad("valueName")
    }

    def repetitionOf(a: Option[ArgIR]): Repetition =
      a match {
        case None      => Repetition.Repeated
        case Some(arg) =>
          arg.repeatable match {
            case None =>
              if (arg.delim.isDefined)
                report.errorAndAbort(
                  "`delim` requires `repeatable = \"delimited\"` or `repeatable = \"either\"`",
                  arg.pos
                )
              Repetition.Repeated
            case Some("repeated") =>
              if (arg.delim.isDefined)
                report.errorAndAbort(
                  "`delim` requires `repeatable = \"delimited\"` or `repeatable = \"either\"`",
                  arg.pos
                )
              Repetition.Repeated
            case Some("delimited") =>
              arg.delim match {
                case Some(d) => Repetition.Delimited(d)
                case None    =>
                  report.errorAndAbort("repeatable = \"delimited\" requires a `delim = '<char>'`", arg.pos)
              }
            case Some("either") =>
              arg.delim match {
                case Some(d) => Repetition.Either(d)
                case None    =>
                  report.errorAndAbort("repeatable = \"either\" requires a `delim = '<char>'`", arg.pos)
              }
            case Some(other) =>
              report.errorAndAbort(
                s"invalid repeatable mode `$other`; expected: repeated, delimited, either",
                arg.pos
              )
          }
      }

    // ---- per-parameter classification --------------------------------------
    m.params.zipWithIndex.foreach { case (p, idx) =>
      val a = p.arg

      if (isPrincipal(p.tpe)) {
        if (a.isDefined)
          perr(
            "auto-injected Principal parameters cannot have @arg annotations because they are not " +
              "part of the tool input schema",
            p
          )
        bindings :+= ParamBindingIR.PrincipalB
      } else if (isStdin(p.tpe) || isStdout(p.tpe)) {
        a.foreach { arg =>
          if (arg.scope.isDefined)
            report.errorAndAbort(
              "an explicit scope is not valid on a stdin/stdout stream parameter",
              arg.pos
            )
          if (arg.argKind.isDefined)
            report.errorAndAbort(
              "`kind = \"flag\"` / \"count-flag\" is not valid on a stdin/stdout stream parameter",
              arg.pos
            )
          rejectRefinements(arg, "a stdin/stdout stream")
          if (arg.hasMinOrMax || arg.unit.isDefined)
            report.errorAndAbort(
              "numeric refinements (`min`/`max`/`bounds`/`unit`) are not valid on a stdin/stdout stream",
              arg.pos
            )
          rejectStructural(arg, "a stdin/stdout stream", Set.empty)
        }
        if (isStdin(p.tpe)) {
          if (stdin.isDefined) perr("duplicate stdin stream parameter", p)
          stdin = Some(StreamSpec(argDoc(a.flatMap(_.doc)), Nil, required = true))
          bindings :+= ParamBindingIR.StdinB
        } else {
          if (stdout.isDefined) perr("duplicate stdout stream parameter", p)
          stdout = Some(StreamSpec(argDoc(a.flatMap(_.doc)), Nil, required = true))
          bindings :+= ParamBindingIR.StdoutB
        }
      } else {
        val scope               = a.flatMap(_.scope)
        val isGlobal            = scope.contains("global")
        val explicit            = scope.filterNot(_ == "global")
        val (baseTpe, optional) = optionArg(p.tpe) match {
          case Some(inner) => (inner, true)
          case None        => (p.tpe, false)
        }
        val argKind = a.flatMap(_.argKind)

        if (isGlobal && (explicit.contains("positional") || explicit.contains("tail")))
          perr("a global parameter cannot be a positional or tail; globals must be options or flags", p)
        if (explicit.exists(s => s == "option" || s == "positional" || s == "tail") && argKind.isDefined)
          perr(
            "a flag kind (`kind = \"flag\"` / `\"count-flag\"`) cannot be combined with an explicit " +
              "option/positional/tail scope",
            p
          )

        val vecItem = seqItem(baseTpe)
        val mapTy   = isMapType(baseTpe)

        val isFlag =
          explicit.contains("flag") || argKind.isDefined ||
            (explicit.isEmpty && !isGlobal && isBool(baseTpe)) ||
            (isGlobal && explicit.isEmpty && argKind.isEmpty && isBool(baseTpe))

        if (isFlag) {
          a.foreach(rejectRefinements(_, "a flag"))
          a.foreach { arg =>
            if (arg.unit.isDefined)
              report.errorAndAbort("numeric refinements (`bounds`/`unit`) are not valid on a flag", arg.pos)
            if (arg.min.isDefined)
              report.errorAndAbort("`min` is not valid on a flag", arg.pos)
          }
          if (optional)
            perr(
              "a flag parameter must not be `Option[_]`: flags are always present (a bool flag has a " +
                "default, a count flag counts occurrences), so optionality has no canonical " +
                "representation; use the bare type (`Boolean` / `Int`)",
              p
            )
          val isCount = argKind.contains("count-flag")
          if (isCount) {
            if (!isInt(baseTpe))
              perr(
                "a count flag parameter must be `Int`: count flags are exposed as a `u32` canonical " +
                  "input field, so any other type would make the metadata disagree with the " +
                  "implementation signature",
                p
              )
            a.foreach(rejectStructural(_, "a count flag", Set("short", "aliases", "env")))
            val maxCount = a.flatMap(_.max).map {
              case ToolLiteral.IntLiteral(v) if v >= 0 && v <= BigInt(Int.MaxValue) => v.toInt
              case other                                                            =>
                report.errorAndAbort(
                  "a count flag `max` must be a non-negative integer literal",
                  a.map(_.pos).getOrElse(pos)
                )
            }
            val spec = FlagSpec(
              long = p.kebab,
              short = a.flatMap(_.short),
              aliases = a.map(_.aliases).getOrElse(Nil),
              doc = argDoc(a.flatMap(_.doc)),
              shape = FlagShape.CountFlag(maxCount),
              envVar = a.flatMap(_.env)
            )
            if (isGlobal) globalFlags :+= spec else bodyFlags :+= spec
          } else {
            if (!isBool(baseTpe))
              perr(
                "a flag parameter must be `Boolean`; for a count flag use `kind = \"count-flag\"` " +
                  "with an `Int` parameter",
                p
              )
            a.foreach { arg =>
              if (arg.max.isDefined)
                report.errorAndAbort(
                  "`max` is only valid on a count flag (`kind = \"count-flag\"`)",
                  arg.pos
                )
            }
            a.foreach(
              rejectStructural(_, "a flag", Set("short", "aliases", "env", "negatable", "default"))
            )
            val default = a.flatMap(_.default) match {
              case None                                 => false
              case Some(ToolLiteral.BoolLiteral(value)) => value
              case Some(_)                              =>
                report.errorAndAbort(
                  "a flag default must be a boolean literal (`true` or `false`)",
                  a.map(_.pos).getOrElse(pos)
                )
            }
            val spec = FlagSpec(
              long = p.kebab,
              short = a.flatMap(_.short),
              aliases = a.map(_.aliases).getOrElse(Nil),
              doc = argDoc(a.flatMap(_.doc)),
              shape = FlagShape.BoolFlag(
                BoolFlagShape(default = default, negatable = a.flatMap(_.negatable).getOrElse(false))
              ),
              envVar = a.flatMap(_.env)
            )
            if (isGlobal) globalFlags :+= spec else bodyFlags :+= spec
          }
          bindings :+= ParamBindingIR.Field(canonicalName(p, m, rootGlobals), p.tpe, countFlag = isCount)
        } else {
          // Value surfaces
          val isTailParam =
            explicit.contains("tail") ||
              (explicit.isEmpty && !isGlobal && vecItem.isDefined && emitAsTail(p, idx))

          val isOptionParam =
            !isTailParam && (
              explicit.contains("option") || isGlobal ||
                (explicit.isEmpty && vecItem.isDefined) ||
                (explicit.isEmpty && mapTy)
            )

          if (isTailParam) {
            if (optional)
              perr(
                "a tail positional must not be `Option[_]`: a tail positional is already variadic " +
                  "(zero or more) and has no representation for an additional optional/absent state, " +
                  "so the `Option` wrapper would be silently dropped; use `Seq[T]` (an empty tail " +
                  "already means none supplied)",
                p
              )
            val item = vecItem.getOrElse(perr("a tail positional must be a `Seq[T]` parameter", p))
            a.foreach(
              rejectStructural(
                _,
                "a tail positional",
                Set("separator", "verbatim", "acceptsStdio", "valueName")
              )
            )
            val minOcc = a
              .flatMap(_.min)
              .map {
                case ToolLiteral.IntLiteral(v) if v >= 0 && v <= BigInt(Int.MaxValue) => v.toInt
                case _                                                                =>
                  report.errorAndAbort(
                    "a tail `min` must be a non-negative integer literal",
                    a.map(_.pos).getOrElse(pos)
                  )
              }
              .getOrElse(0)
            val maxOcc = a.flatMap(_.max).map {
              case ToolLiteral.IntLiteral(v) if v >= 0 && v <= BigInt(Int.MaxValue) => v.toInt
              case _                                                                =>
                report.errorAndAbort(
                  "a tail `max` must be a non-negative integer literal",
                  a.map(_.pos).getOrElse(pos)
                )
            }
            val tailIR = TailIR(
              name = p.kebab,
              doc = argDoc(a.flatMap(_.doc)),
              valueName = a.flatMap(_.valueName),
              item = item,
              refinements = a.map(_.refinements(includeMinMax = false)).getOrElse(ToolArgRefinements.empty),
              min = minOcc,
              max = maxOcc,
              separator = a.flatMap(_.separator),
              verbatim = a.exists(_.verbatim),
              acceptsStdio = a.exists(_.acceptsStdio)
            )

            val explicitTail = explicit.contains("tail")
            if (explicitTail && isInheritedRedecl(p)) {
              // An explicit tail that redeclares an inherited global is lowered
              // to a repeatable-list option surrogate; the authored tail spec is
              // preserved in the plan so promotion is lossless.
              val surrogate = OptionIR(
                long = p.kebab,
                short = None,
                aliases = a.map(_.aliases).getOrElse(Nil),
                doc = tailIR.doc,
                valueName = tailIR.valueName,
                shape = ShapeIR.RList(item, Repetition.Repeated),
                refinements = ToolArgRefinements.empty,
                default = None,
                required = false,
                env = None
              )
              bodyOptions :+= surrogate
              plan :+= PlanIR.Vec(
                name = p.kebab,
                explicitTail = true,
                optionalVec = false,
                hasMinOrMaxAttr = a.exists(_.hasMinOrMax),
                authoredTailSurrogate = Some(tailIR),
                laterOptionNames = Nil // filled in after the loop
              )
            } else {
              if (tail.isDefined)
                perr("a command may have at most one tail positional", p)
              tail = Some(tailIR)
              plan :+= PlanIR.Vec(
                name = p.kebab,
                explicitTail = explicitTail,
                optionalVec = false,
                hasMinOrMaxAttr = a.exists(_.hasMinOrMax),
                authoredTailSurrogate = None,
                laterOptionNames = Nil
              )
            }
            bindings :+= ParamBindingIR.Field(canonicalName(p, m, rootGlobals), p.tpe)
          } else if (isOptionParam) {
            if (isGlobal && explicit.contains("positional"))
              perr("a global argument cannot be a positional; use an option or flag", p)
            if (isGlobal && explicit.contains("tail"))
              perr("a global argument cannot be a tail positional", p)

            val shapeIR =
              vecItem match {
                case Some(item) =>
                  a.foreach(
                    rejectStructural(
                      _,
                      "a repeatable list option",
                      Set("short", "aliases", "env", "required", "repeatable", "delim", "default", "valueName")
                    )
                  )
                  ShapeIR.RList(item, repetitionOf(a))
                case None if mapTy =>
                  a.foreach { arg =>
                    rejectRefinements(arg, "a map option")
                    if (arg.hasMinOrMax || arg.unit.isDefined)
                      report.errorAndAbort(
                        "numeric refinements (`min`/`max`/`bounds`/`unit`) are not valid on a map option",
                        arg.pos
                      )
                  }
                  a.foreach(
                    rejectStructural(
                      _,
                      "a map option",
                      Set("short", "aliases", "env", "required", "repeatable", "delim", "default", "valueName")
                    )
                  )
                  ShapeIR.RMap(baseTpe, repetitionOf(a))
                case None =>
                  a.foreach(
                    rejectStructural(
                      _,
                      "a scalar option",
                      Set("short", "aliases", "env", "required", "optionalScalar", "default", "valueName")
                    )
                  )
                  ShapeIR.Scalar(baseTpe, a.exists(_.optionalScalar))
              }

            val includeMinMax = shapeIR match {
              case _: ShapeIR.RMap => false
              case _               => true
            }
            val optionIR = OptionIR(
              long = p.kebab,
              short = a.flatMap(_.short),
              aliases = a.map(_.aliases).getOrElse(Nil),
              doc = argDoc(a.flatMap(_.doc)),
              valueName = a.flatMap(_.valueName),
              shape = shapeIR,
              refinements = a.map(_.refinements(includeMinMax)).getOrElse(ToolArgRefinements.empty),
              default = a.flatMap(_.default),
              required = !optional && a.flatMap(_.required).getOrElse(false),
              env = a.flatMap(_.env)
            )
            if (isGlobal) globalOptions :+= optionIR
            else {
              bodyOptions :+= optionIR
              // A body-scoped Seq/Option[Seq] option is a positional-eligible
              // candidate (a repeatable-list projection of a possible tail).
              if (vecItem.isDefined && !explicit.contains("option"))
                plan :+= PlanIR.Vec(
                  name = p.kebab,
                  explicitTail = false,
                  optionalVec = optional,
                  hasMinOrMaxAttr = a.exists(_.hasMinOrMax),
                  authoredTailSurrogate = None,
                  laterOptionNames = Nil
                )
            }
            bindings :+= ParamBindingIR.Field(canonicalName(p, m, rootGlobals), p.tpe)
          } else {
            // Fixed positional
            a.foreach(
              rejectStructural(
                _,
                "a positional",
                Set("required", "default", "acceptsStdio", "valueName")
              )
            )
            val required = !optional && a.flatMap(_.required).getOrElse(true)
            // Inherited re-declarations are excluded from the ordering rules
            // (they are removed/rejected by normalization); only genuine body
            // positionals constrain ordering.
            if (!isInheritedRedecl(p)) {
              if (tail.isDefined)
                perr(
                  "a fixed positional cannot appear after a tail positional; the tail positional " +
                    "must be the last positional",
                  p
                )
              if (required && sawOptionalPositional)
                perr(
                  "a required positional cannot appear after an optional positional; optional " +
                    "positionals must be trailing",
                  p
                )
              if (!required) sawOptionalPositional = true
            }
            fixed :+= PositionalIR(
              name = p.kebab,
              doc = argDoc(a.flatMap(_.doc)),
              valueName = a.flatMap(_.valueName),
              tpe = baseTpe,
              refinements = a.map(_.refinements(includeMinMax = true)).getOrElse(ToolArgRefinements.empty),
              default = a.flatMap(_.default),
              required = required,
              acceptsStdio = a.exists(_.acceptsStdio)
            )
            plan :+= PlanIR.Plain(p.kebab)
            bindings :+= ParamBindingIR.Field(canonicalName(p, m, rootGlobals), p.tpe)
          }
        }
      }
    }

    // Fill in each vec candidate's later-option names: the long names of the
    // body options declared after it, in declaration order.
    val optionOrder = bodyOptions.map(_.long)
    plan = plan.map {
      case v: PlanIR.Vec =>
        val after = optionOrder.dropWhile(_ != v.name) match {
          case Nil         => optionOrder.filter(_ != v.name) // candidate not itself an option (holds tail slot)
          case _ :: laters => laters
        }
        // For a candidate holding the tail slot, "later options" are the body
        // options declared after its parameter position.
        val laterNames =
          if (bodyOptions.exists(_.long == v.name)) after
          else {
            val paramIdx = m.params.indexWhere(_.kebab == v.name)
            m.params.drop(paramIdx + 1).map(_.kebab).filter(k => optionOrder.contains(k))
          }
        v.copy(laterOptionNames = laterNames)
      case other => other
    }

    ClassifiedCommand(
      method = m,
      globalOptions = globalOptions,
      globalFlags = globalFlags,
      fixed = fixed,
      tail = tail,
      bodyOptions = bodyOptions,
      bodyFlags = bodyFlags,
      stdin = stdin,
      stdout = stdout,
      plan = plan,
      bindings = bindings
    )
  }

  /**
   * The canonical input field name for one method parameter: its kebab-cased
   * name, unless (on a non-root command) its surface intersects a root global,
   * in which case the root global's long name is canonical.
   */
  private def canonicalName(
    p: ParamIR,
    m: MethodIR,
    rootGlobals: List[(String, List[String])]
  ): String =
    if (m.isRoot) p.kebab
    else {
      val names = p.kebab :: p.arg.map(_.aliases).getOrElse(Nil)
      rootGlobals.find(g => surfaceIntersects(names, g)).map(_._1).getOrElse(p.kebab)
    }

  /**
   * Classifies a subtree method's parameters as propagating parent globals.
   * Every parameter must project to a global option or flag.
   */
  final case class ClassifiedSubtree(
    method: MethodIR,
    childTrait: TypeRepr,
    parentOptions: List[OptionIR],
    parentFlags: List[FlagSpec]
  )

  def classifySubtree(ir: ToolIR, m: MethodIR): ClassifiedSubtree = {
    val pos = m.sym.pos.getOrElse(Position.ofMacroExpansion)
    if (m.annotations.isDefined)
      report.errorAndAbort(
        "annotations are not supported on a subtree method (the model places annotations on a " +
          "command body)",
        pos
      )
    if (m.constraints.nonEmpty || m.resultAttr.isDefined)
      report.errorAndAbort("@constraint / @result are not supported on a subtree method", pos)

    // Classify as an isolated global-only command; anything that does not
    // project to a global option/flag is rejected.
    val globalized = m.copy(params = m.params.map { p =>
      if (isPrincipal(p.tpe) || isStdin(p.tpe) || isStdout(p.tpe))
        report.errorAndAbort(
          "a subtree method parameter must project to a global option or flag",
          p.sym.pos.getOrElse(pos)
        )
      p.arg.flatMap(_.scope) match {
        case Some("global") | None =>
          val withGlobal = p.arg match {
            case Some(a) => Some(a.copy(scope = Some("global")))
            case None    =>
              Some(
                ArgIR(
                  key = p.kebab,
                  pos = pos,
                  scope = Some("global"),
                  argKind = None,
                  pathKind = None,
                  short = None,
                  aliases = Nil,
                  env = None,
                  required = None,
                  negatable = None,
                  optionalScalar = false,
                  repeatable = None,
                  delim = None,
                  default = None,
                  defaultIsBool = false,
                  separator = None,
                  verbatim = false,
                  acceptsStdio = false,
                  regex = None,
                  minLength = None,
                  maxLength = None,
                  direction = None,
                  mime = None,
                  schemes = None,
                  min = None,
                  max = None,
                  unit = None,
                  doc = None,
                  valueName = None
                )
              )
          }
          p.copy(arg = withGlobal)
        case Some("flag") | Some("option") =>
          // explicit option/flag on a subtree param is treated as global
          val a = p.arg.get
          p.copy(arg =
            Some(
              a.copy(
                scope = Some("global"),
                argKind = a.argKind.orElse {
                  if (a.scope.contains("flag")) Some("flag") else None
                }
              )
            )
          )
        case Some(_) =>
          report.errorAndAbort(
            "a subtree method parameter must project to a global option or flag",
            p.sym.pos.getOrElse(pos)
          )
      }
    })

    val classified = classifyCommand(ir, globalized, Nil)
    if (
      classified.fixed.nonEmpty || classified.tail.isDefined || classified.bodyOptions.nonEmpty ||
      classified.bodyFlags.nonEmpty || classified.stdin.isDefined || classified.stdout.isDefined
    )
      report.errorAndAbort("a subtree method parameter must project to a global option or flag", pos)

    ClassifiedSubtree(
      method = m,
      childTrait = m.subtreeTrait.getOrElse(
        report.errorAndAbort("subtree method must return a @toolDefinition trait", pos)
      ),
      parentOptions = classified.globalOptions,
      parentFlags = classified.globalFlags
    )
  }

  // -------------------------------------------------------------------------
  // Result / error metadata
  // -------------------------------------------------------------------------

  final case class ResultIR(okType: TypeRepr, formatters: List[Formatter], defaultFormatter: String)

  def resultOf(m: MethodIR): Option[ResultIR] = {
    val pos = m.sym.pos.getOrElse(Position.ofMacroExpansion)
    val ok  = m.shape.kind match {
      case ReturnKind.Value(t)          => Some(t)
      case ReturnKind.EitherK(_, okOpt) => okOpt
      case ReturnKind.UnitK             => None
    }
    ok match {
      case None =>
        if (m.resultAttr.isDefined)
          report.errorAndAbort(
            "@result(...) is not valid on a method with a unit success type: there is no result " +
              "value to format",
            pos
          )
        None
      case Some(t) =>
        val (names, default) = m.resultAttr.getOrElse((Nil, None))
        val formatterNames   = if (names.isEmpty) List("default") else names
        val defaultFormatter = default.getOrElse(formatterNames.head)
        Some(
          ResultIR(
            okType = t,
            formatters = formatterNames.map(n => Formatter(n, Doc.empty)),
            defaultFormatter = defaultFormatter
          )
        )
    }
  }

  final case class ErrorCaseIR(
    caseSym: Symbol,
    name: String,
    doc: Doc,
    kind: ErrorKind,
    exitCode: Int,
    payload: Option[TypeRepr],
    payloadFieldName: Option[String]
  )

  /**
   * Parses the cases of a tool error enum (or sealed trait): each case must
   * carry `@error(kind, exitCode)` and at most one payload field. A `Unit`
   * error type has no cases.
   */
  def errorCasesOf(errType: TypeRepr, pos: Position): List[ErrorCaseIR] = {
    if (isUnit(errType)) return Nil
    val sym            = errType.dealias.typeSymbol
    val isEnumOrSealed =
      sym.flags
        .is(Flags.Enum) || (sym.flags.is(Flags.Sealed) && (sym.flags.is(Flags.Trait) || sym.flags.is(Flags.Abstract)))
    if (!isEnumOrSealed)
      report.errorAndAbort(
        s"a tool error type must be an enum (or sealed trait) with @error-annotated cases, " +
          s"found: ${sym.fullName}",
        pos
      )
    val children = sym.children
    children.map { child =>
      val errAnns = annotationsOf(child, ErrorFQN)
      if (errAnns.isEmpty)
        report.errorAndAbort(
          s"case `${child.name}` is missing @error(kind = \"...\", exitCode = ...)",
          child.pos.getOrElse(pos)
        )
      if (errAnns.length > 1)
        report.errorAndAbort(
          "duplicate @error(...); a case may have at most one",
          child.pos.getOrElse(pos)
        )
      val values = annotationValues(errAnns.head, List("kind", "exitCode"))
      val kind   = values.get("kind") match {
        case None    => report.errorAndAbort("@error is missing `kind`", child.pos.getOrElse(pos))
        case Some(t) =>
          constString(t, "kind", errAnns.head.pos) match {
            case "usage" | "usage-error"     => ErrorKind.UsageError
            case "runtime" | "runtime-error" => ErrorKind.RuntimeError
            case other                       =>
              report.errorAndAbort(
                s"invalid error kind `$other`; expected `usage-error` or `runtime-error`",
                errAnns.head.pos
              )
          }
      }
      val exitCode = values.get("exitCode") match {
        case None    => report.errorAndAbort("@error is missing `exitCode`", child.pos.getOrElse(pos))
        case Some(t) =>
          val v = constInt(t, "exitCode", errAnns.head.pos)
          if (v < 0 || v > 255)
            report.errorAndAbort("exitCode must be an integer literal in 0..=255", errAnns.head.pos)
          v
      }

      val caseFields: List[Symbol] =
        if (child.isClassDef && !child.flags.is(Flags.Module)) child.caseFields
        else Nil
      val payload = caseFields match {
        case Nil          => None
        case field :: Nil =>
          Some((field.name, fieldTypeOf(child, field, pos)))
        case _ =>
          report.errorAndAbort(
            "a tool error case may have at most one field; wrap multiple values in a case class",
            child.pos.getOrElse(pos)
          )
      }

      ErrorCaseIR(
        caseSym = child,
        name = kebabCase(child.name),
        doc = docOf(child),
        kind = kind,
        exitCode = exitCode,
        payload = payload.map(_._2),
        payloadFieldName = payload.map(_._1)
      )
    }
  }

  private def fieldTypeOf(caseSym: Symbol, field: Symbol, pos: Position): TypeRepr =
    field.tree match {
      case v: ValDef => v.tpt.tpe
      case _         =>
        report.errorAndAbort(s"unable to read payload field type of case ${caseSym.name}", pos)
    }
}
