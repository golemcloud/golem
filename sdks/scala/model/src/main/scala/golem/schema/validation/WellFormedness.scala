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

package golem.schema.validation

import golem.schema._
import golem.schema.SchemaTypeBody._

import java.util.regex.Pattern
import scala.collection.mutable

sealed trait NumericRestrictionError extends Product with Serializable {
  def message: String; override def toString: String = message
}
object NumericRestrictionError {
  case object EmptyStored extends NumericRestrictionError {
    def message: String = "numeric restriction set is empty (must be None)"
  }
  case object FamilyMismatch extends NumericRestrictionError {
    def message: String = "numeric bound family does not match the numeric type"
  }
  case object BoundOutOfRange extends NumericRestrictionError {
    def message: String = "numeric bound does not fit the numeric type's range"
  }
  case object NonFiniteFloat extends NumericRestrictionError {
    def message: String = "numeric float bound must be finite"
  }
  case object FloatNotRoundTrippable extends NumericRestrictionError {
    def message: String = "f32 numeric bound does not round-trip through f32"
  }
  case object MinGreaterThanMax extends NumericRestrictionError {
    def message: String = "numeric min bound is greater than max bound"
  }
}

sealed trait SchemaError extends Product with Serializable {
  def message: String; override def toString: String = message
}
object SchemaError {
  final case class DuplicateTypeId(id: String) extends SchemaError { def message = s"duplicate type id `$id`"       }
  final case class DanglingRef(id: String)     extends SchemaError { def message = s"dangling type reference `$id`" }
  final case class RecursiveAlias(id: String)  extends SchemaError {
    def message = s"type reference `$id` forms a reference cycle with no concrete type"
  }
  case object EmptyVariant                            extends SchemaError { def message = "variant has no cases"     }
  case object EmptyEnum                               extends SchemaError { def message = "enum has no cases"        }
  case object EmptyUnion                              extends SchemaError { def message = "union has no branches"    }
  case object EmptyFlags                              extends SchemaError { def message = "flags has no entries"     }
  final case class DuplicateFieldName(name: String)   extends SchemaError { def message = s"duplicate field `$name`" }
  final case class DuplicateVariantCase(name: String) extends SchemaError {
    def message = s"duplicate variant case `$name`"
  }
  final case class DuplicateEnumCase(name: String) extends SchemaError { def message = s"duplicate enum case `$name`" }
  final case class DuplicateFlagName(name: String) extends SchemaError { def message = s"duplicate flag `$name`"      }
  final case class DuplicateUnionTag(tag: String)  extends SchemaError {
    def message = s"duplicate union branch tag `$tag`"
  }
  case object MapKeyNotPrimitive                                              extends SchemaError { def message = "map key must be a primitive type" }
  case object FixedListZeroLength                                             extends SchemaError { def message = "fixed-list length must be > 0"    }
  case object QuantityMinGreaterThanMax                                       extends SchemaError { def message = "quantity min is greater than max" }
  final case class QuantityMinUnitMismatch(baseUnit: String, minUnit: String) extends SchemaError {
    def message = s"quantity min unit mismatch: base `$baseUnit`, min `$minUnit`"
  }
  final case class QuantityMaxUnitMismatch(baseUnit: String, maxUnit: String) extends SchemaError {
    def message = s"quantity max unit mismatch: base `$baseUnit`, max `$maxUnit`"
  }
  final case class QuantityComparisonOverflow(baseUnit: String) extends SchemaError {
    def message = s"quantity range comparison overflowed in base unit `$baseUnit`"
  }
  final case class UnionStringRuleOnNonStringBody(tag: String) extends SchemaError {
    def message = s"union branch `$tag` uses a string-pattern rule but body is not string-shaped"
  }
  final case class UnionFieldRuleOnNonRecordBody(tag: String) extends SchemaError {
    def message = s"union branch `$tag` uses a field rule but body is not record-shaped"
  }
  final case class UnionFieldEqualsLiteralOnNonStringField(tag: String, fieldName: String) extends SchemaError {
    def message =
      s"union branch `$tag` references field `$fieldName` for a literal comparison but the field is not string-shaped"
  }
  final case class UnionFieldRuleMissingField(tag: String, fieldName: String) extends SchemaError {
    def message = s"union branch `$tag` references record field `$fieldName` that does not exist"
  }
  final case class UnionAmbiguousDiscriminators(tagA: String, tagB: String, reason: String) extends SchemaError {
    def message = s"union branches `$tagA` and `$tagB` have overlapping discriminators ($reason)"
  }
  final case class UnionUnsatisfiableFieldAbsent(tag: String, fieldName: String) extends SchemaError {
    def message = s"union branch `$tag` uses field-absent on `$fieldName` but the record body declares that field"
  }
  final case class InvalidRegex(tag: String, regex: String, regexMessage: String) extends SchemaError {
    def message = s"union branch `$tag` regex `$regex` failed to compile: $regexMessage"
  }
  final case class InvalidTextRegex(regex: String, regexMessage: String) extends SchemaError {
    def message = s"text regex `$regex` failed to compile: $regexMessage"
  }
  case object TextLengthRangeInverted                                        extends SchemaError { def message = "text min-length is greater than max-length" }
  case object BinaryByteRangeInverted                                        extends SchemaError { def message = "binary min-bytes is greater than max-bytes" }
  final case class InvalidNumericRestriction(error: NumericRestrictionError) extends SchemaError {
    def message = s"invalid numeric restriction: $error"
  }
  final case class NullableNesting(inner: String) extends SchemaError {
    def message =
      s"option<$inner> is invalid because the inner type is also nullable; use a variant with explicit cases to distinguish absence from explicit none"
  }
}

object WellFormedness {
  import SchemaError._
  def validateGraph(graph: SchemaGraph): Either[List[SchemaError], Unit] =
    validateRootAndDefs(graph, graph.root, includeDefs = true)
  def validateRootType(graph: SchemaGraph, tpe: SchemaType): Either[List[SchemaError], Unit] =
    validateRootAndDefs(graph, tpe, includeDefs = false)
  private def validateRootAndDefs(graph: SchemaGraph, root: SchemaType, includeDefs: Boolean) = {
    val errors = mutable.ListBuffer.empty[SchemaError]
    if (includeDefs) {
      val seen = mutable.Set.empty[String];
      graph.defs.keys.foreach(id => if (!seen.add(id)) errors += DuplicateTypeId(id));
      graph.defs.values.foreach(d => checkType(graph, d.body, errors))
    }
    checkType(graph, root, errors)
    if (errors.isEmpty) Right(()) else Left(errors.toList)
  }
  private def checkType(graph: SchemaGraph, t: SchemaType, errors: mutable.ListBuffer[SchemaError]): Unit =
    t.body match {
      case RefType(id) =>
        RefResolution.resolveRef(graph, t).left.foreach {
          case RefResolutionError.DanglingRef(x)  => errors += DanglingRef(x);
          case RefResolutionError.RecursiveRef(x) => errors += RecursiveAlias(x)
        }
      case RecordType(fields) =>
        dup(fields.map(_.name), DuplicateFieldName, errors); fields.foreach(f => checkType(graph, f.body, errors))
      case VariantType(cases) =>
        if (cases.isEmpty) errors += EmptyVariant; dup(cases.map(_.name), DuplicateVariantCase, errors);
        cases.flatMap(_.payload).foreach(checkType(graph, _, errors))
      case EnumType(cases)     => if (cases.isEmpty) errors += EmptyEnum; dup(cases, DuplicateEnumCase, errors)
      case FlagsType(names)    => if (names.isEmpty) errors += EmptyFlags; dup(names, DuplicateFlagName, errors)
      case TupleType(es)       => es.foreach(checkType(graph, _, errors))
      case ListType(e)         => checkType(graph, e, errors)
      case FixedListType(e, l) => if (l == 0) errors += FixedListZeroLength; checkType(graph, e, errors)
      case MapType(k, v)       =>
        if (classifyMapKey(graph, k) == 1) errors += MapKeyNotPrimitive; checkType(graph, k, errors);
        checkType(graph, v, errors)
      case OptionType(e) =>
        if (isNullable(graph, e, Set.empty)) errors += NullableNesting(describeNullable(e)); checkType(graph, e, errors)
      case ResultType(ok, err) => ok.foreach(checkType(graph, _, errors)); err.foreach(checkType(graph, _, errors))
      case QuantityType(spec)  => checkQuantity(spec, errors)
      case TextType(r)         =>
        if (r.minLength.exists(min => r.maxLength.exists(min > _))) errors += TextLengthRangeInverted;
        r.regex.foreach(rx =>
          try Pattern.compile(rx)
          catch { case e: Exception => errors += InvalidTextRegex(rx, e.getMessage) }
        )
      case BinaryType(r) => if (r.minBytes.exists(min => r.maxBytes.exists(min > _))) errors += BinaryByteRangeInverted
      case UnionType(bs) => validateUnion(graph, bs, errors)
      case FutureType(e) => e.foreach(checkType(graph, _, errors))
      case StreamType(e) => e.foreach(checkType(graph, _, errors))
      case SecretType(s) => checkType(graph, s.inner, errors)
      case S8Type(r)     => checkNum(r, S8, errors); case S16Type(r)  => checkNum(r, S16, errors);
      case S32Type(r)    => checkNum(r, S32, errors); case S64Type(r) => checkNum(r, S64, errors)
      case U8Type(r)     => checkNum(r, U8, errors); case U16Type(r)  => checkNum(r, U16, errors);
      case U32Type(r)    => checkNum(r, U32, errors); case U64Type(r) => checkNum(r, U64, errors)
      case F32Type(r)    => checkNum(r, F32, errors); case F64Type(r) => checkNum(r, F64, errors)
      case _             => ()
    }
  private def dup(xs: List[String], mk: String => SchemaError, e: mutable.ListBuffer[SchemaError]): Unit = {
    val s = mutable.Set.empty[String]; xs.foreach(x => if (!s.add(x)) e += mk(x))
  }
  private sealed trait Repr; private case object S8 extends Repr; private case object S16 extends Repr;
  private case object S32                           extends Repr; private case object S64 extends Repr; private case object U8  extends Repr;
  private case object U16                           extends Repr; private case object U32 extends Repr; private case object U64 extends Repr;
  private case object F32                           extends Repr; private case object F64 extends Repr
  private def checkNum(r: Option[NumericRestrictions], repr: Repr, e: mutable.ListBuffer[SchemaError]): Unit =
    r.foreach(validateNum(_, repr).left.foreach(x => e += InvalidNumericRestriction(x)))
  private def validateNum(r: NumericRestrictions, repr: Repr): Either[NumericRestrictionError, Unit] = {
    import NumericRestrictionError._
    if (r.min.isEmpty && r.max.isEmpty && r.unit.forall(_.isEmpty)) return Left(EmptyStored)
    def fam(b: NumericBound) = b match {
      case NumericBound.Signed(_) => 0; case NumericBound.Unsigned(_) => 1; case NumericBound.FloatBits(_) => 2
    }
    def rf                       = repr match { case S8 | S16 | S32 | S64 => 0; case U8 | U16 | U32 | U64 => 1; case _ => 2 }
    def inRange(b: NumericBound) = (b, repr) match {
      case (NumericBound.Signed(v), S8)        => v >= Byte.MinValue && v <= Byte.MaxValue;
      case (NumericBound.Signed(v), S16)       => v >= Short.MinValue && v <= Short.MaxValue;
      case (NumericBound.Signed(v), S32)       => v >= Int.MinValue && v <= Int.MaxValue;
      case (NumericBound.Signed(_), S64)       => true;
      case (NumericBound.Unsigned(v), U8)      => java.lang.Long.compareUnsigned(v, 255L) <= 0;
      case (NumericBound.Unsigned(v), U16)     => java.lang.Long.compareUnsigned(v, 65535L) <= 0;
      case (NumericBound.Unsigned(v), U32)     => java.lang.Long.compareUnsigned(v, 0xffffffffL) <= 0;
      case (NumericBound.Unsigned(_), U64)     => true;
      case (NumericBound.FloatBits(bits), F32) => {
        val d = java.lang.Double.longBitsToDouble(bits); d.isFinite && d.toFloat.toDouble == d
      };
      case (NumericBound.FloatBits(bits), F64) => java.lang.Double.longBitsToDouble(bits).isFinite; case _ => false
    }
    for (b <- List(r.min, r.max).flatten) {
      if (fam(b) != rf) return Left(FamilyMismatch);
      if (!inRange(b))
        return Left(
          if (
            rf == 2 && b.isInstanceOf[NumericBound.FloatBits] && !java.lang.Double
              .longBitsToDouble(b.asInstanceOf[NumericBound.FloatBits].value)
              .isFinite
          ) NonFiniteFloat
          else if (repr == F32) FloatNotRoundTrippable
          else BoundOutOfRange
        )
    }
    (r.min, r.max) match {
      case (Some(a), Some(b)) if cmp(a, b).exists(_ > 0) => Left(MinGreaterThanMax); case _ => Right(())
    }
  }
  private def cmp(a: NumericBound, b: NumericBound): Option[Int] = (a, b) match {
    case (NumericBound.Signed(x), NumericBound.Signed(y))       => Some(java.lang.Long.compare(x, y));
    case (NumericBound.Unsigned(x), NumericBound.Unsigned(y))   => Some(java.lang.Long.compareUnsigned(x, y));
    case (NumericBound.FloatBits(x), NumericBound.FloatBits(y)) =>
      Some(java.lang.Double.compare(java.lang.Double.longBitsToDouble(x), java.lang.Double.longBitsToDouble(y)));
    case _ => None
  }
  private def classifyMapKey(g: SchemaGraph, t: SchemaType): Int =
    RefResolution.resolveRef(g, t).fold(_ => 2, x => if (isPrimitive(x.body)) 0 else 1)
  private def isPrimitive(b: SchemaTypeBody) = b match {
    case BoolType | S8Type(_) | S16Type(_) | S32Type(_) | S64Type(_) | U8Type(_) | U16Type(_) | U32Type(_) |
        U64Type(_) | F32Type(_) | F64Type(_) | CharType | StringType =>
      true;
    case _ => false
  }
  private def checkQuantity(s: QuantitySpec, e: mutable.ListBuffer[SchemaError]): Unit = {
    if (s.min.exists(_.unit != s.baseUnit)) e += QuantityMinUnitMismatch(s.baseUnit, s.min.get.unit);
    if (s.max.exists(_.unit != s.baseUnit)) e += QuantityMaxUnitMismatch(s.baseUnit, s.max.get.unit);
    for (a <- s.min; b <- s.max if a.unit == s.baseUnit && b.unit == s.baseUnit) qle(a, b) match {
      case Some(false) => e += QuantityMinGreaterThanMax; case None => e += QuantityComparisonOverflow(s.baseUnit);
      case _           => ()
    }
  }
  private def qle(a: QuantityValue, b: QuantityValue): Option[Boolean] = try {
    val c = math.max(a.scale, b.scale);
    Some(
      BigInt(a.mantissa) * BigInt(10).pow(math.max(c - a.scale, 0)) <= BigInt(b.mantissa) * BigInt(10).pow(
        math.max(c - b.scale, 0)
      )
    )
  } catch { case _: Throwable => None }
  private def validateUnion(g: SchemaGraph, bs: List[UnionBranch], e: mutable.ListBuffer[SchemaError]): Unit = {
    if (bs.isEmpty) e += EmptyUnion; dup(bs.map(_.tag), DuplicateUnionTag, e);
    bs.foreach { b => checkBranch(g, b, e); checkType(g, b.body, e) };
    for (i <- bs.indices; j <- i + 1 until bs.length)
      overlap(bs(i).discriminator, bs(j).discriminator).foreach(r =>
        e += UnionAmbiguousDiscriminators(bs(i).tag, bs(j).tag, r)
      )
  }
  private sealed trait Shape; private case object Str        extends Shape;
  private final case class Rec(fields: List[NamedFieldType]) extends Shape; private case object Other extends Shape;
  private case object Unres                                  extends Shape
  private def shape(g: SchemaGraph, t: SchemaType, seen: Set[String] = Set.empty): Shape = t.body match {
    case RefType(id) if seen(id)                             => Unres;
    case RefType(id)                                         => g.defs.get(id).map(d => shape(g, d.body, seen + id)).getOrElse(Unres);
    case StringType | TextType(_) | UrlType(_) | PathType(_) => Str; case RecordType(fs) => Rec(fs); case _ => Other
  }
  private def checkBranch(g: SchemaGraph, b: UnionBranch, e: mutable.ListBuffer[SchemaError]): Unit =
    b.discriminator match {
      case DiscriminatorRule.Prefix(_) | DiscriminatorRule.Suffix(_) | DiscriminatorRule.Contains(_) =>
        if (shape(g, b.body) != Str && shape(g, b.body) != Unres) e += UnionStringRuleOnNonStringBody(b.tag);
      case DiscriminatorRule.Regex(rx) =>
        if (shape(g, b.body) != Str && shape(g, b.body) != Unres) e += UnionStringRuleOnNonStringBody(b.tag);
        if (rx.isEmpty) e += InvalidRegex(b.tag, rx, "regex must be non-empty")
        else
          try Pattern.compile(rx)
          catch { case ex: Exception => e += InvalidRegex(b.tag, rx, ex.getMessage) };
      case DiscriminatorRule.FieldEquals(fd) =>
        shape(g, b.body) match {
          case Rec(fs) =>
            fs.find(_.name == fd.fieldName) match {
              case None    => e += UnionFieldRuleMissingField(b.tag, fd.fieldName);
              case Some(f) =>
                if (fd.literal.nonEmpty && shape(g, f.body) != Str && shape(g, f.body) != Unres)
                  e += UnionFieldEqualsLiteralOnNonStringField(b.tag, fd.fieldName)
            };
          case Unres => (); case _ => e += UnionFieldRuleOnNonRecordBody(b.tag)
        };
      case DiscriminatorRule.FieldAbsent(n) =>
        shape(g, b.body) match {
          case Rec(fs) => if (fs.exists(_.name == n)) e += UnionUnsatisfiableFieldAbsent(b.tag, n); case Unres => ();
          case _       => e += UnionFieldRuleOnNonRecordBody(b.tag)
        }
    }
  private def overlap(a: DiscriminatorRule, b: DiscriminatorRule): Option[String] = (a, b) match {
    case (DiscriminatorRule.Prefix(x), DiscriminatorRule.Prefix(y)) if x.isEmpty && y.isEmpty =>
      Some("both prefixes are empty");
    case (DiscriminatorRule.Prefix(x), DiscriminatorRule.Prefix(y)) if x.isEmpty =>
      Some(s"empty prefix overlaps any other prefix `$y`");
    case (DiscriminatorRule.Prefix(x), DiscriminatorRule.Prefix(y)) if y.isEmpty =>
      Some(s"empty prefix overlaps any other prefix `$x`");
    case (DiscriminatorRule.Prefix(x), DiscriminatorRule.Prefix(y)) if x.startsWith(y) || y.startsWith(x) =>
      Some(s"prefix `$x` and prefix `$y` overlap");
    case (DiscriminatorRule.Regex(x), DiscriminatorRule.Regex(y)) if x == y                       => Some(s"both branches share regex `$x`");
    case (DiscriminatorRule.Contains(x), DiscriminatorRule.Contains(y)) if x.isEmpty || y.isEmpty =>
      Some("empty contains substring matches every string");
    case _ => None
  }
  private def isNullable(g: SchemaGraph, t: SchemaType, seen: Set[String]): Boolean = t.body match {
    case OptionType(_)           => true; case UnionType(bs)                                             => bs.exists(b => isNullable(g, b.body, seen));
    case RefType(id) if seen(id) => false;
    case RefType(id)             => g.defs.get(id).exists(d => isNullable(g, d.body, seen + id)); case _ => false
  }
  private def describeNullable(t: SchemaType): String = t.body match {
    case OptionType(_) => "option<_>"; case UnionType(_) => "union"; case RefType(id) => s"ref `$id`";
    case _             => "nullable"
  }
}
