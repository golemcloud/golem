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

package golem.tool

import golem.schema._
import golem.schema.validation.{RefResolution, SchemaError, ValueValidation, WellFormedness}

import scala.collection.immutable.ListMap
import scala.collection.mutable

/**
 * Graph-level helpers shared by tool validation, encoding, canonical-input
 * synthesis and inherited-global reconciliation: the derived value graphs of
 * options/flags/tails, `value-is` comparand construction and compatibility,
 * structural graph checks, and the shape-matching used by de-projection.
 */
private[tool] object ToolGraphs {

  /**
   * The whole collected value type of an option (used to validate an option's
   * `default`): a repeatable-list collects into `list<item>`, a repeatable-map
   * into its map node; scalar/optional-scalar use the value type directly.
   * Definition graphs are preserved so refs still resolve.
   */
  def optionCollectedGraph(shape: ExtendedOptionShape): SchemaGraph =
    shape match {
      case ExtendedOptionShape.Scalar(g)         => g
      case ExtendedOptionShape.OptionalScalar(g) => g
      case ExtendedOptionShape.RepeatableList(r) => listWrapperGraph(r.itemType)
      case ExtendedOptionShape.RepeatableMap(r)  => r.mapType
    }

  /**
   * The full input value type of an option (used for the "no variant in input
   * position" check). A repeatable-list stores its element type; a
   * repeatable-map stores the whole map node so both key and value are reached.
   */
  def optionInputGraph(shape: ExtendedOptionShape): SchemaGraph =
    shape match {
      case ExtendedOptionShape.Scalar(g)         => g
      case ExtendedOptionShape.OptionalScalar(g) => g
      case ExtendedOptionShape.RepeatableList(r) => r.itemType
      case ExtendedOptionShape.RepeatableMap(r)  => r.mapType
    }

  /**
   * The authored, self-contained [[SchemaGraph]] backing an option's value
   * type, regardless of how the option collects on the command line.
   */
  def optionAuthoredGraph(shape: ExtendedOptionShape): SchemaGraph =
    shape match {
      case ExtendedOptionShape.Scalar(g)         => g
      case ExtendedOptionShape.OptionalScalar(g) => g
      case ExtendedOptionShape.RepeatableList(r) => r.itemType
      case ExtendedOptionShape.RepeatableMap(r)  => r.mapType
    }

  /**
   * Wrap a graph's root in a `list`, preserving the original definitions so any
   * `Ref` in the element type still resolves.
   */
  def listWrapperGraph(item: SchemaGraph): SchemaGraph =
    SchemaGraph(item.defs, SchemaType(SchemaTypeBody.ListType(item.root)))

  /**
   * The derived input-record field type for a flag (`bool` for a bool-flag,
   * `u32` for a count-flag). Flags carry no author-supplied value type, so this
   * is used only by canonical-input synthesis; a `value-is` literal against a
   * flag is rejected rather than checked against this type.
   */
  def flagGraph(flag: FlagSpec): SchemaGraph = {
    val body = flag.shape match {
      case _: FlagShape.BoolFlag  => SchemaTypeBody.BoolType
      case _: FlagShape.CountFlag => SchemaTypeBody.U32Type(None)
    }
    SchemaGraph(ListMap.empty, SchemaType(body))
  }

  /**
   * How a `value-is` literal is matched against its comparand graph. The
   * distinction is whether the referenced surface *collects* multiple CLI
   * occurrences into a container: a collecting surface compares exactly one
   * collected occurrence, never the whole container.
   */
  sealed trait ValueIsMode extends Product with Serializable
  object ValueIsMode {

    /**
     * The literal must be a valid value for the comparand graph exactly, with
     * no element/value relaxation. Used for *collecting* surfaces — a
     * repeatable-list option, a repeatable-map option, or a tail positional —
     * whose comparand is already the per-occurrence type.
     */
    case object Exact extends ValueIsMode

    /**
     * The literal may be a valid value for the comparand graph as a whole, or —
     * after peeling leading `option` wrappers — for exactly one element of a
     * list/fixed-list comparand or one value of a map comparand. Used for
     * *non-collecting* value surfaces: scalar / optional-scalar options and
     * fixed positionals, whose declared value is the comparand itself.
     */
    case object WholeOrOnePeel extends ValueIsMode
  }

  /**
   * A `value-is` comparand: the graph a literal is matched against, plus the
   * [[ValueIsMode]] controlling whether the one-level element/value relaxation
   * applies.
   */
  final case class ValueIsComparand(graph: SchemaGraph, mode: ValueIsMode)

  /**
   * The `value-is` comparand recorded for a referenceable name. A
   * value-carrying name maps to a typed comparand; a name whose declared type
   * cannot yield a comparable value (a repeatable-map whose map type does not
   * resolve to a map) is [[ValueComparand.BlockedByTypeError]] so `value-is`
   * checking is suppressed and the underlying type error is reported by
   * validation instead of a misleading cascading mismatch. A name absent from
   * the scope's comparand map is a flag (no value type), against which a
   * `value-is` is a genuine mismatch.
   */
  sealed trait ValueComparand extends Product with Serializable
  object ValueComparand {
    final case class Typed(comparand: ValueIsComparand) extends ValueComparand
    case object BlockedByTypeError                      extends ValueComparand
  }

  /**
   * The `value-is` comparand for an option, keyed by whether the option
   * collects occurrences. A scalar / optional-scalar option is non-collecting:
   * its declared value graph is matched with the whole-or-one-peel relaxation.
   * A repeatable-list option collects into a list, so its comparand is the
   * per-occurrence item type matched exactly; a repeatable-map option collects
   * into a map, so its comparand is the per-entry map *value* type matched
   * exactly. A repeatable-map whose map type does not resolve to a map yields
   * [[ValueComparand.BlockedByTypeError]].
   */
  def optionValueIsComparand(shape: ExtendedOptionShape): ValueComparand =
    shape match {
      case ExtendedOptionShape.Scalar(g) =>
        ValueComparand.Typed(ValueIsComparand(g, ValueIsMode.WholeOrOnePeel))
      case ExtendedOptionShape.OptionalScalar(g) =>
        ValueComparand.Typed(ValueIsComparand(g, ValueIsMode.WholeOrOnePeel))
      case ExtendedOptionShape.RepeatableList(r) =>
        ValueComparand.Typed(ValueIsComparand(r.itemType, ValueIsMode.Exact))
      case ExtendedOptionShape.RepeatableMap(r) if resolvesToMap(r.mapType) =>
        ValueComparand.Typed(ValueIsComparand(mapValueGraph(r.mapType), ValueIsMode.Exact))
      case _: ExtendedOptionShape.RepeatableMap =>
        ValueComparand.BlockedByTypeError
    }

  /**
   * Whether a comparand graph is structurally sound (no dangling references or
   * pure-alias cycles). When it is not, validation reports the schema error, so
   * `value-is` resolution must not also report a cascading mismatch against a
   * graph that cannot be resolved.
   */
  def comparandGraphIsSound(graph: SchemaGraph): Boolean =
    WellFormedness.validateGraph(graph).isRight

  /**
   * The per-entry value comparand for a map: a graph whose root is the map's
   * value type, with the map graph's definitions preserved so any `Ref` in the
   * value type still resolves. Falls back to the original graph when the root
   * does not resolve to a `Map`.
   */
  def mapValueGraph(map: SchemaGraph): SchemaGraph =
    RefResolution.resolveRef(map, map.root) match {
      case Right(SchemaType(SchemaTypeBody.MapType(_, value), _)) => SchemaGraph(map.defs, value)
      case _                                                      => map
    }

  /** Whether the graph's root resolves (through any `Ref`s) to a `Map`. */
  def resolvesToMap(map: SchemaGraph): Boolean =
    RefResolution.resolveRef(map, map.root) match {
      case Right(SchemaType(_: SchemaTypeBody.MapType, _)) => true
      case _                                               => false
    }

  /**
   * Validate a self-contained per-argument [[SchemaGraph]] for structural
   * well-formedness: every embedded type is well-formed, every ref resolves
   * within the graph's own defs, and inline restrictions are valid. A dangling
   * reference is surfaced as a position-aware
   * [[ToolBuildError.UnresolvedTypeRef]]; any other failure becomes
   * [[ToolBuildError.IllFormedSchema]].
   *
   * Closedness (no dangling refs) is also what makes per-argument validation
   * equivalent to validation against the merged tool schema: merging only
   * unions defs (rejecting id collisions with conflicting bodies), so once each
   * embedded graph is well-formed and closed, resolving a ref or validating a
   * default / `value-is` literal against the local graph yields the same result
   * as against the merged graph.
   */
  def checkGraphClosed(graph: SchemaGraph, position: String): Either[ToolBuildError, Unit] =
    WellFormedness.validateGraph(graph) match {
      case Right(_)     => Right(())
      case Left(errors) =>
        // Report the first error deterministically (validateGraph collects in
        // a stable discovery order), preferring the precise dangling-ref
        // variant.
        val first = errors.collectFirst { case e: SchemaError.DanglingRef => e }
          .orElse(errors.headOption)
        first match {
          case Some(SchemaError.DanglingRef(id)) =>
            Left(ToolBuildError.UnresolvedTypeRef(position, id))
          case Some(other) =>
            Left(ToolBuildError.IllFormedSchema(position, other.message))
          case None => Right(())
        }
    }

  def validateDefault(value: SchemaValue, graph: SchemaGraph): Either[ToolBuildError, Unit] =
    ValueValidation.validateValue(graph, graph.root, value) match {
      case Right(_)     => Right(())
      case Left(errors) => Left(ToolBuildError.DefaultTypeMismatch(errors.mkString(", ")))
    }

  /**
   * Whether a `value-is` literal is compatible with its [[ValueIsComparand]].
   *
   * The literal is always compatible if it is a valid value for the comparand
   * graph as a whole. For a [[ValueIsMode.WholeOrOnePeel]] comparand (a
   * non-collecting value surface) it is *also* compatible — under the WIT "any
   * element / entry equals this literal" relaxation — if it is a valid value
   * for the element type of a list-shaped, or the value type of a map-shaped,
   * (optionally `option`-wrapped) comparand. A [[ValueIsMode.Exact]] comparand
   * (a collecting surface, whose graph is already the per-occurrence type) gets
   * no relaxation.
   */
  def valueIsCompatible(comparand: ValueIsComparand, value: SchemaValue): Boolean = {
    val graph = comparand.graph
    if (ValueValidation.validateValue(graph, graph.root, value).isRight) return true
    if (comparand.mode == ValueIsMode.Exact) return false

    def peel(tpe: SchemaType): Option[SchemaType] =
      RefResolution.resolveRef(graph, tpe).toOption.flatMap { resolved =>
        resolved.body match {
          case SchemaTypeBody.OptionType(inner) => peel(inner)
          case _                                => Some(resolved)
        }
      }

    peel(graph.root) match {
      case None         => false
      case Some(peeled) =>
        peeled.body match {
          case SchemaTypeBody.ListType(element) =>
            ValueValidation.validateValue(graph, element, value).isRight
          case SchemaTypeBody.FixedListType(element, _) =>
            ValueValidation.validateValue(graph, element, value).isRight
          case SchemaTypeBody.MapType(_, mapValue) =>
            ValueValidation.validateValue(graph, mapValue, value).isRight
          case _ => false
        }
    }
  }

  def graphReachesVariant(graph: SchemaGraph): Boolean =
    typeReachesVariant(graph, graph.root, mutable.Set.empty)

  /**
   * Returns true if `tpe` (resolving named references against `graph`) reaches
   * a variant type; `visited` guards recursive graphs.
   */
  private def typeReachesVariant(
    graph: SchemaGraph,
    tpe: SchemaType,
    visited: mutable.Set[String]
  ): Boolean = {
    tpe.body match {
      case SchemaTypeBody.RefType(id) if !visited.add(id) => return false
      case _                                              => ()
    }
    RefResolution.resolveRef(graph, tpe) match {
      case Left(_)         => false
      case Right(resolved) =>
        resolved.body match {
          case _: SchemaTypeBody.VariantType     => true
          case SchemaTypeBody.RecordType(fields) =>
            fields.exists(f => typeReachesVariant(graph, f.body, visited))
          case SchemaTypeBody.ListType(element) =>
            typeReachesVariant(graph, element, visited)
          case SchemaTypeBody.FixedListType(element, _) =>
            typeReachesVariant(graph, element, visited)
          case SchemaTypeBody.OptionType(inner) =>
            typeReachesVariant(graph, inner, visited)
          case SchemaTypeBody.MapType(key, value) =>
            typeReachesVariant(graph, key, visited) || typeReachesVariant(graph, value, visited)
          case SchemaTypeBody.TupleType(elements) =>
            elements.exists(e => typeReachesVariant(graph, e, visited))
          case SchemaTypeBody.ResultType(ok, err) =>
            ok.exists(t => typeReachesVariant(graph, t, visited)) ||
            err.exists(t => typeReachesVariant(graph, t, visited))
          case SchemaTypeBody.UnionType(branches) =>
            branches.exists(b => typeReachesVariant(graph, b.body, visited))
          case SchemaTypeBody.FutureType(Some(t)) => typeReachesVariant(graph, t, visited)
          case SchemaTypeBody.StreamType(Some(t)) => typeReachesVariant(graph, t, visited)
          case _                                  => false
        }
    }
  }

  /**
   * Maximum recursion depth for structural shape comparison; deeper than this
   * the comparison gives up and reports "not a match". Reporting "not a match"
   * on exhaustion is the safe direction: a non-match between two same-named
   * declarations surfaces as an explicit
   * [[ToolBuildError.InheritedGlobalConflict]] rather than silently dropping a
   * local parameter that might actually differ.
   */
  private val ShapeMatchMaxDepth: Int = 32

  /**
   * Whether two canonical input value graphs describe the same value *shape*,
   * ignoring metadata and validation restrictions (docs, numeric/text bounds,
   * etc.) but honoring structure and exact primitive representation. References
   * are resolved against their respective graphs.
   *
   * Recursive (cyclic) graphs are compared coinductively: when the same pair of
   * referenced definitions is reached again along a path, the two shapes are
   * assumed to match (the cycle has already been established structurally). The
   * per-pair memo is what guarantees termination; the depth counter is a
   * defensive secondary guard for pathologically deep finite types.
   */
  def schemaShapesMatch(a: SchemaGraph, b: SchemaGraph): Boolean =
    schemaTypesMatch(a, a.root, b, b.root, ShapeMatchMaxDepth, mutable.Set.empty)

  private def schemaTypesMatch(
    aGraph: SchemaGraph,
    aTy: SchemaType,
    bGraph: SchemaGraph,
    bTy: SchemaType,
    depth: Int,
    visiting: mutable.Set[(String, String)]
  ): Boolean = {
    // Break recursion at reference boundaries before resolving: revisiting the
    // same pair of named definitions means we have already entered comparing
    // them, so the recursive shapes coincide along this path.
    (aTy.body, bTy.body) match {
      case (SchemaTypeBody.RefType(aId), SchemaTypeBody.RefType(bId)) =>
        if (!visiting.add((aId, bId))) return true
      case _ => ()
    }
    val resolved = for {
      a <- RefResolution.resolveRef(aGraph, aTy).toOption
      b <- RefResolution.resolveRef(bGraph, bTy).toOption
    } yield (a, b)
    resolved match {
      case None         => false
      case Some((a, b)) =>
        if (depth == 0) return false
        val next = depth - 1
        import SchemaTypeBody._

        def rec(x: SchemaType, y: SchemaType): Boolean =
          schemaTypesMatch(aGraph, x, bGraph, y, next, visiting)

        def optRec(x: Option[SchemaType], y: Option[SchemaType]): Boolean =
          (x, y) match {
            case (None, None)         => true
            case (Some(xt), Some(yt)) => rec(xt, yt)
            case _                    => false
          }

        (a.body, b.body) match {
          case (ListType(ea), ListType(eb))                   => rec(ea, eb)
          case (FixedListType(ea, la), FixedListType(eb, lb)) => la == lb && rec(ea, eb)
          case (OptionType(ia), OptionType(ib))               => rec(ia, ib)
          case (MapType(ka, va), MapType(kb, vb))             => rec(ka, kb) && rec(va, vb)
          case (TupleType(ea), TupleType(eb))                 =>
            ea.length == eb.length && ea.zip(eb).forall { case (x, y) => rec(x, y) }
          case (RecordType(fa), RecordType(fb)) =>
            fa.length == fb.length && fa.zip(fb).forall { case (x, y) =>
              x.name == y.name && rec(x.body, y.body)
            }
          case (VariantType(ca), VariantType(cb)) =>
            ca.length == cb.length && ca.zip(cb).forall { case (x, y) =>
              x.name == y.name && optRec(x.payload, y.payload)
            }
          case (UnionType(ba), UnionType(bb)) =>
            ba.length == bb.length && ba.zip(bb).forall { case (x, y) =>
              x.tag == y.tag && x.discriminator == y.discriminator && rec(x.body, y.body)
            }
          case (EnumType(ca), EnumType(cb))   => ca == cb
          case (FlagsType(fa), FlagsType(fb)) => fa == fb
          // Rich leaf types whose spec carries *type identity* (not just
          // refinable validation restrictions) must compare those identity
          // fields; otherwise a leaf could de-project an inherited global onto
          // a genuinely different Scala type — e.g. `Quantity[Meters]` vs
          // `Quantity[Seconds]`. Identity here means the parts of the spec
          // derived from the Scala type (not overlaid by `@arg`): the quantity
          // unit set, the secret payload type and category, the quota-token
          // resource, and the unstructured text languages / binary MIME sets.
          // Pure validation restrictions (numeric/text/url bounds, path
          // direction/kind — all `@arg`-refinable) stay ignored per this
          // method's contract.
          case (QuantityType(sa), QuantityType(sb)) =>
            sa.baseUnit == sb.baseUnit &&
            strSetsMatch(
              effectiveQuantityUnits(sa.baseUnit, sa.allowedSuffixes),
              effectiveQuantityUnits(sb.baseUnit, sb.allowedSuffixes)
            )
          case (SecretType(sa), SecretType(sb)) =>
            sa.category == sb.category && rec(sa.inner, sb.inner)
          case (QuotaTokenType(sa), QuotaTokenType(sb)) =>
            sa.resourceName == sb.resourceName
          case (TextType(ra), TextType(rb)) =>
            optStrSetsMatch(ra.languages, rb.languages)
          case (BinaryType(ra), BinaryType(rb)) =>
            optStrSetsMatch(ra.mimeTypes, rb.mimeTypes)
          // A plain `string` and a `text` differ only by the latter carrying
          // refinable restrictions (regex/min/max), which `@arg` overlays via
          // refineText (the only `string`→`text` promotion). So an inherited
          // refined-`String` global (`text`) and a leaf redeclaring the same
          // plain `String` describe the same shape and must de-project. The
          // exception is a `languages`-restricted `text`, which reflects a
          // different Scala type (`UnstructuredText`) and stays incompatible
          // with a plain `String`.
          case (StringType, TextType(restrictions))     => restrictions.languages.isEmpty
          case (TextType(restrictions), StringType)     => restrictions.languages.isEmpty
          case (ResultType(oa, ea), ResultType(ob, eb)) =>
            optRec(oa, ob) && optRec(ea, eb)
          case (FutureType(ia), FutureType(ib)) => optRec(ia, ib)
          case (StreamType(ia), StreamType(ib)) => optRec(ia, ib)
          // Primitives and the remaining rich leaf types (incl. distinct
          // numeric widths/signs, `url`, `path`, `datetime`, `duration`) are
          // compared by kind, which already ignores their refinable
          // restrictions.
          case (x, y) => sameKind(x, y)
        }
    }
  }

  /**
   * Whether two schema type bodies are the same structural kind (the
   * discriminant comparison of the fallback arm; restrictions are ignored).
   */
  private def sameKind(a: SchemaTypeBody, b: SchemaTypeBody): Boolean = {
    def kind(body: SchemaTypeBody): Int =
      body match {
        case _: SchemaTypeBody.RefType        => 0
        case SchemaTypeBody.BoolType          => 1
        case _: SchemaTypeBody.S8Type         => 2
        case _: SchemaTypeBody.S16Type        => 3
        case _: SchemaTypeBody.S32Type        => 4
        case _: SchemaTypeBody.S64Type        => 5
        case _: SchemaTypeBody.U8Type         => 6
        case _: SchemaTypeBody.U16Type        => 7
        case _: SchemaTypeBody.U32Type        => 8
        case _: SchemaTypeBody.U64Type        => 9
        case _: SchemaTypeBody.F32Type        => 10
        case _: SchemaTypeBody.F64Type        => 11
        case SchemaTypeBody.CharType          => 12
        case SchemaTypeBody.StringType        => 13
        case _: SchemaTypeBody.RecordType     => 14
        case _: SchemaTypeBody.VariantType    => 15
        case _: SchemaTypeBody.EnumType       => 16
        case _: SchemaTypeBody.FlagsType      => 17
        case _: SchemaTypeBody.TupleType      => 18
        case _: SchemaTypeBody.ListType       => 19
        case _: SchemaTypeBody.FixedListType  => 20
        case _: SchemaTypeBody.MapType        => 21
        case _: SchemaTypeBody.OptionType     => 22
        case _: SchemaTypeBody.ResultType     => 23
        case _: SchemaTypeBody.TextType       => 24
        case _: SchemaTypeBody.BinaryType     => 25
        case _: SchemaTypeBody.PathType       => 26
        case _: SchemaTypeBody.UrlType        => 27
        case SchemaTypeBody.DatetimeType      => 28
        case SchemaTypeBody.DurationType      => 29
        case _: SchemaTypeBody.QuantityType   => 30
        case _: SchemaTypeBody.UnionType      => 31
        case _: SchemaTypeBody.SecretType     => 32
        case _: SchemaTypeBody.QuotaTokenType => 33
        case _: SchemaTypeBody.FutureType     => 34
        case _: SchemaTypeBody.StreamType     => 35
      }
    kind(a) == kind(b)
  }

  /**
   * Whether two string collections describe the same *set* of values (order-
   * and duplicate-insensitive). Used to compare rich-type identity fields that
   * are authored as ordered lists but semantically unordered.
   */
  private def strSetsMatch(a: List[String], b: List[String]): Boolean =
    a.toSet == b.toSet

  /**
   * Set comparison for an optional restriction (`None` = unrestricted). An
   * unrestricted side never matches a restricted side: they describe different
   * accepted value sets.
   */
  private def optStrSetsMatch(a: Option[List[String]], b: Option[List[String]]): Boolean =
    (a, b) match {
      case (None, None)         => true
      case (Some(xs), Some(ys)) => strSetsMatch(xs, ys)
      case _                    => false
    }

  /**
   * The set of units a quantity type accepts: its explicit `allowedSuffixes`,
   * or just the canonical `baseUnit` when no suffixes are declared. Two
   * quantities with the same base unit but different accepted unit sets are not
   * interchangeable, so de-projection must treat them as distinct.
   */
  private def effectiveQuantityUnits(baseUnit: String, allowedSuffixes: List[String]): List[String] =
    if (allowedSuffixes.isEmpty) List(baseUnit) else allowedSuffixes
}
