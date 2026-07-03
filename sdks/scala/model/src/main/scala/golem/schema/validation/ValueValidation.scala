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
import golem.schema.SchemaValue._

import java.util.regex.Pattern
import scala.collection.mutable

sealed trait ValuePathSegment extends Product with Serializable {
  override def toString: String = this match {
    case ValuePathSegment.Field(n)       => ".field(\"" + n + "\")"; case ValuePathSegment.Index(i) => s".index($i)";
    case ValuePathSegment.VariantPayload => ".variant_payload"; case ValuePathSegment.OptionInner   => ".option_inner";
    case ValuePathSegment.ResultOk       => ".ok"; case ValuePathSegment.ResultErr                  => ".err";
    case ValuePathSegment.UnionBody      => ".union_body"; case ValuePathSegment.MapKey(i)          => s".map_key($i)";
    case ValuePathSegment.MapValue(i)    => s".map_value($i)"
  }
}
object ValuePathSegment {
  final case class Field(name: String)  extends ValuePathSegment;
  final case class Index(index: Int)    extends ValuePathSegment; case object VariantPayload extends ValuePathSegment;
  case object OptionInner               extends ValuePathSegment; case object ResultOk       extends ValuePathSegment;
  case object ResultErr                 extends ValuePathSegment; case object UnionBody      extends ValuePathSegment;
  final case class MapKey(index: Int)   extends ValuePathSegment;
  final case class MapValue(index: Int) extends ValuePathSegment
}
final case class ValuePath(segments: List[ValuePathSegment] = Nil) { override def toString: String = segments.mkString }

sealed trait ResultSide extends Product with Serializable
object ResultSide { case object Ok extends ResultSide; case object Err extends ResultSide }
sealed trait ValueError extends Product with Serializable {
  def path: ValuePath; def message: String; override def toString: String = message
}
object ValueError {
  final case class ShapeMismatch(path: ValuePath, expected: String, found: String) extends ValueError {
    def message = s"shape mismatch at $path: expected $expected, found $found"
  }
  final case class VariantCaseOutOfRange(path: ValuePath, caseIndex: Int, caseCount: Int) extends ValueError {
    def message = s"variant case index $caseIndex at $path is out of range (case count: $caseCount)"
  }
  final case class EnumCaseOutOfRange(path: ValuePath, caseIndex: Int, caseCount: Int) extends ValueError {
    def message = s"enum case index $caseIndex at $path is out of range (case count: $caseCount)"
  }
  final case class RecordArityMismatch(path: ValuePath, expected: Int, found: Int) extends ValueError {
    def message = s"record at $path has $found field(s), expected $expected"
  }
  final case class TupleArityMismatch(path: ValuePath, expected: Int, found: Int) extends ValueError {
    def message = s"tuple at $path has $found element(s), expected $expected"
  }
  final case class FlagsArityMismatch(path: ValuePath, expected: Int, found: Int) extends ValueError {
    def message = s"flags value at $path has $found bit(s), expected $expected"
  }
  final case class FixedListLengthMismatch(path: ValuePath, expected: Int, found: Int) extends ValueError {
    def message = s"fixed-list at $path has $found element(s), expected $expected"
  }
  final case class DanglingRef(path: ValuePath, typeId: String) extends ValueError {
    def message = s"dangling ref `$typeId` at $path (no such named definition)"
  }
  final case class RecursiveRef(path: ValuePath, typeId: String) extends ValueError {
    def message = s"ref chain at $path loops back to `$typeId` without reaching a structural shape"
  }
  final case class UnionUnknownTag(path: ValuePath, tag: String) extends ValueError {
    def message = s"union value at $path carries tag `$tag` that does not match any branch"
  }
  final case class UnionDiscriminatorMismatch(path: ValuePath, tag: String) extends ValueError {
    def message = s"union value at $path does not satisfy branch `$tag` discriminator"
  }
  final case class VariantPayloadPresenceMismatch(path: ValuePath, expectedSome: Boolean) extends ValueError {
    def message =
      s"variant payload presence mismatch at $path: " + (if (expectedSome)
                                                           "schema expects a payload, value carries none"
                                                         else "schema expects no payload, value carries one")
  }
  final case class ResultPayloadPresenceMismatch(path: ValuePath, expectedSome: Boolean, side: ResultSide)
      extends ValueError {
    def message =
      s"result ${if (side == ResultSide.Ok) "ok" else "err"} payload presence mismatch at $path: " + (if (expectedSome)
                                                                                                        "schema expects a payload, value carries none"
                                                                                                      else
                                                                                                        "schema expects no payload, value carries one")
  }
  final case class OptionInnerPresenceMismatch(path: ValuePath) extends ValueError {
    def message = s"option-value presence inconsistent at $path"
  }
  final case class TextLanguageNotAllowed(path: ValuePath, language: String) extends ValueError {
    def message = s"text value at $path carries language `$language` not in the allow-list"
  }
  final case class TextTooShort(path: ValuePath, min: Int, found: Int) extends ValueError {
    def message = s"text value at $path has $found char(s), below min-length $min"
  }
  final case class TextTooLong(path: ValuePath, max: Int, found: Int) extends ValueError {
    def message = s"text value at $path has $found char(s), above max-length $max"
  }
  final case class TextRegexMismatch(path: ValuePath, regex: String) extends ValueError {
    def message = s"text value at $path does not match required regex `$regex`"
  }
  final case class BinaryMimeNotAllowed(path: ValuePath, mimeType: String) extends ValueError {
    def message = s"binary value at $path carries mime-type `$mimeType` not in the allow-list"
  }
  final case class BinaryTooSmall(path: ValuePath, min: Int, found: Int) extends ValueError {
    def message = s"binary value at $path has $found byte(s), below min-bytes $min"
  }
  final case class BinaryTooLarge(path: ValuePath, max: Int, found: Int) extends ValueError {
    def message = s"binary value at $path has $found byte(s), above max-bytes $max"
  }
  final case class PathEmpty(path: ValuePath)                                  extends ValueError { def message = s"path value at $path is empty" }
  final case class PathExtensionNotAllowed(path: ValuePath, extension: String) extends ValueError {
    def message = s"path value at $path has extension `$extension` not in the allow-list"
  }
  final case class UrlEmpty(path: ValuePath)                                extends ValueError { def message = s"url value at $path is empty" }
  final case class UrlInvalid(path: ValuePath, url: String, reason: String) extends ValueError {
    def message = s"url value at $path (`$url`) is not a valid URL: $reason"
  }
  final case class UrlSchemeNotAllowed(path: ValuePath, scheme: String) extends ValueError {
    def message = s"url value at $path has scheme `$scheme` not in the allow-list"
  }
  final case class UrlHostNotAllowed(path: ValuePath, host: String) extends ValueError {
    def message = s"url value at $path has host `$host` not in the allow-list"
  }
  final case class UrlHostMissing(path: ValuePath) extends ValueError {
    def message = s"url value at $path has no host (allow-list requires one)"
  }
  final case class QuantityUnitNotAllowed(path: ValuePath, unit: String) extends ValueError {
    def message = s"quantity value at $path has unit `$unit` which is not allowed"
  }
  final case class QuantityOutOfRange(path: ValuePath, reason: String) extends ValueError {
    def message = s"quantity value at $path is out of range ($reason)"
  }
  final case class NumericOutOfRange(path: ValuePath, reason: String) extends ValueError {
    def message = s"numeric value at $path is out of range ($reason)"
  }
  final case class SecretCategoryMismatch(path: ValuePath, expected: String, found: Option[String]) extends ValueError {
    def message = s"secret value at $path expected category `$expected`, found `${found.getOrElse("<none>")}`"
  }
  final case class QuotaTokenResourceMismatch(path: ValuePath, expected: String, found: String) extends ValueError {
    def message = s"quota-token value at $path expected resource `$expected`, found `$found`"
  }
}

object ValueValidation {
  import ValueError._
  def validateValue(graph: SchemaGraph, tpe: SchemaType, value: SchemaValue): Either[List[ValueError], Unit] = {
    val e = mutable.ListBuffer.empty[ValueError]; check(graph, tpe, value, Nil, e);
    if (e.isEmpty) Right(()) else Left(e.toList)
  }
  private def check(
    g: SchemaGraph,
    t: SchemaType,
    v: SchemaValue,
    p: List[ValuePathSegment],
    e: mutable.ListBuffer[ValueError]
  ): Unit = resolve(g, t, p, e).foreach { rt =>
    (rt.body, v) match {
      case (BoolType, BoolValue(_)) | (CharType, CharValue(_)) | (StringType, StringValue(_)) |
          (DatetimeType, DatetimeValue(_)) | (DurationType, DurationValue(_)) =>
        ()
      case (S8Type(r), S8Value(x))   => checkNum(r, NumericBound.Signed(x), p, e);
      case (S16Type(r), S16Value(x)) => checkNum(r, NumericBound.Signed(x), p, e);
      case (S32Type(r), S32Value(x)) => checkNum(r, NumericBound.Signed(x), p, e);
      case (S64Type(r), S64Value(x)) => checkNum(r, NumericBound.Signed(x), p, e)
      case (U8Type(r), U8Value(x))   => checkNum(r, NumericBound.Unsigned(x.toLong), p, e);
      case (U16Type(r), U16Value(x)) => checkNum(r, NumericBound.Unsigned(x.toLong), p, e);
      case (U32Type(r), U32Value(x)) => checkNum(r, NumericBound.Unsigned(x), p, e);
      case (U64Type(r), U64Value(x)) => checkNum(r, NumericBound.Unsigned(x), p, e)
      case (F32Type(r), F32Value(x)) =>
        checkNum(r, NumericBound.FloatBits(java.lang.Double.doubleToRawLongBits(x.toDouble)), p, e);
      case (F64Type(r), F64Value(x)) =>
        checkNum(r, NumericBound.FloatBits(java.lang.Double.doubleToRawLongBits(x)), p, e)
      case (TextType(r), TextValue(txt, lang)) =>
        lang.foreach(l => if (r.languages.exists(!_.contains(l))) e += TextLanguageNotAllowed(path(p), l));
        val len = txt.codePointCount(0, txt.length);
        r.minLength.foreach(m => if (len < m) e += TextTooShort(path(p), m, len));
        r.maxLength.foreach(m => if (len > m) e += TextTooLong(path(p), m, len));
        r.regex.foreach(rx =>
          try if (!Pattern.compile(rx).matcher(txt).find()) e += TextRegexMismatch(path(p), rx)
          catch { case _: Exception => () }
        )
      case (BinaryType(r), BinaryValue(bytes, mime)) =>
        mime.foreach(m => if (r.mimeTypes.exists(!_.contains(m))) e += BinaryMimeNotAllowed(path(p), m));
        r.minBytes.foreach(m => if (bytes.size < m) e += BinaryTooSmall(path(p), m, bytes.size));
        r.maxBytes.foreach(m => if (bytes.size > m) e += BinaryTooLarge(path(p), m, bytes.size))
      case (PathType(s), PathValue(x)) =>
        if (x.isEmpty) e += PathEmpty(path(p))
        else
          for (ext <- fileExt(x); allowed <- s.allowedExtensions if !allowed.contains(ext))
            e += PathExtensionNotAllowed(path(p), ext)
      case (UrlType(r), UrlValue(u))                => checkUrl(r, u, p, e)
      case (QuantityType(s), QuantityValueNode(q))  => checkQuantity(s, q, p, e)
      case (SecretType(_), SecretValue(_))          => ()
      case (QuotaTokenType(_), QuotaTokenHandle(_)) => ()
      case (RecordType(fs), RecordValue(vs))        =>
        if (fs.length != vs.length) e += RecordArityMismatch(path(p), fs.length, vs.length)
        else fs.zip(vs).foreach { case (f, x) => check(g, f.body, x, p :+ ValuePathSegment.Field(f.name), e) }
      case (VariantType(cs), VariantValue(i, pay)) =>
        if (i < 0 || i >= cs.length) e += VariantCaseOutOfRange(path(p), i, cs.length)
        else
          (cs(i).payload, pay) match {
            case (Some(t), Some(x)) => check(g, t, x, p :+ ValuePathSegment.VariantPayload, e); case (None, None) => ();
            case (Some(_), None)    => e += VariantPayloadPresenceMismatch(path(p), true);
            case (None, Some(_))    => e += VariantPayloadPresenceMismatch(path(p), false)
          }
      case (EnumType(cs), EnumValue(i))    => if (i < 0 || i >= cs.length) e += EnumCaseOutOfRange(path(p), i, cs.length)
      case (FlagsType(ns), FlagsValue(fs)) =>
        if (ns.length != fs.length) e += FlagsArityMismatch(path(p), ns.length, fs.length)
      case (TupleType(ts), TupleValue(vs)) =>
        if (ts.length != vs.length) e += TupleArityMismatch(path(p), ts.length, vs.length)
        else ts.zip(vs).zipWithIndex.foreach { case ((t, x), i) => check(g, t, x, p :+ ValuePathSegment.Index(i), e) }
      case (ListType(t), ListValue(vs)) =>
        vs.zipWithIndex.foreach { case (x, i) => check(g, t, x, p :+ ValuePathSegment.Index(i), e) }
      case (FixedListType(t, l), FixedListValue(vs)) =>
        if (l != vs.length) e += FixedListLengthMismatch(path(p), l, vs.length)
        else vs.zipWithIndex.foreach { case (x, i) => check(g, t, x, p :+ ValuePathSegment.Index(i), e) }
      case (MapType(k, vt), MapValue(entries)) =>
        entries.zipWithIndex.foreach { case (en, i) =>
          check(g, k, en.key, p :+ ValuePathSegment.MapKey(i), e);
          check(g, vt, en.value, p :+ ValuePathSegment.MapValue(i), e)
        }
      case (OptionType(t), OptionValue(Some(x)))                   => check(g, t, x, p :+ ValuePathSegment.OptionInner, e);
      case (OptionType(_), OptionValue(None))                      => ()
      case (ResultType(ok, err), ResultValue(SchemaResult.Ok(x)))  => payload(g, ok, x, p, ResultSide.Ok, e);
      case (ResultType(ok, err), ResultValue(SchemaResult.Err(x))) => payload(g, err, x, p, ResultSide.Err, e)
      case (UnionType(bs), UnionValue(tag, body))                  =>
        bs.find(_.tag == tag) match {
          case None    => e += UnionUnknownTag(path(p), tag);
          case Some(b) =>
            val pp = p :+ ValuePathSegment.UnionBody; check(g, b.body, body, pp, e);
            if (!disc(g, b, body)) e += UnionDiscriminatorMismatch(path(pp), tag)
        }
      case _ => e += ShapeMismatch(path(p), typeName(rt.body), shapeName(v))
    }
  }
  private def payload(
    g: SchemaGraph,
    t: Option[SchemaType],
    v: Option[SchemaValue],
    p: List[ValuePathSegment],
    side: ResultSide,
    e: mutable.ListBuffer[ValueError]
  ): Unit = (t, v) match {
    case (Some(tt), Some(x)) =>
      check(g, tt, x, p :+ (if (side == ResultSide.Ok) ValuePathSegment.ResultOk else ValuePathSegment.ResultErr), e);
    case (None, None)    => (); case (Some(_), None) => e += ResultPayloadPresenceMismatch(path(p), true, side);
    case (None, Some(_)) => e += ResultPayloadPresenceMismatch(path(p), false, side)
  }
  private def resolve(
    g: SchemaGraph,
    t: SchemaType,
    p: List[ValuePathSegment],
    e: mutable.ListBuffer[ValueError]
  ): Option[SchemaType] = {
    var cur = t; var hops = 0;
    while (cur.body.isInstanceOf[RefType]) {
      val id = cur.body.asInstanceOf[RefType].id;
      if (hops > g.defs.size) { e += RecursiveRef(path(p), id); return None }; hops += 1;
      g.defs.get(id) match { case Some(d) => cur = d.body; case None => e += DanglingRef(path(p), id); return None }
    };
    Some(cur)
  }
  private def path(p: List[ValuePathSegment]) = ValuePath(p)
  private def typeName(b: SchemaTypeBody)     = b match {
    case RefType(_)     => "ref"; case BoolType          => "bool"; case S8Type(_)           => "s8"; case S16Type(_)   => "s16";
    case S32Type(_)     => "s32"; case S64Type(_)        => "s64"; case U8Type(_)            => "u8"; case U16Type(_)   => "u16";
    case U32Type(_)     => "u32"; case U64Type(_)        => "u64"; case F32Type(_)           => "f32"; case F64Type(_)  => "f64";
    case CharType       => "char"; case StringType       => "string"; case RecordType(_)     => "record";
    case VariantType(_) => "variant"; case EnumType(_)   => "enum"; case FlagsType(_)        => "flags";
    case TupleType(_)   => "tuple"; case ListType(_)     => "list"; case FixedListType(_, _) => "fixed-list";
    case MapType(_, _)  => "map"; case OptionType(_)     => "option"; case ResultType(_, _)  => "result";
    case TextType(_)    => "text"; case BinaryType(_)    => "binary"; case PathType(_)       => "path"; case UrlType(_) => "url";
    case DatetimeType   => "datetime"; case DurationType => "duration"; case QuantityType(_) => "quantity";
    case UnionType(_)   => "union"; case SecretType(_)   => "secret"; case QuotaTokenType(_) => "quota-token";
    case FutureType(_)  => "future"; case StreamType(_)  => "stream"
  }
  private def shapeName(v: SchemaValue) =
    v.getClass.getSimpleName.stripSuffix("Value").replace("ValueNode", "").toLowerCase
  private def checkNum(
    r: Option[NumericRestrictions],
    v: NumericBound,
    p: List[ValuePathSegment],
    e: mutable.ListBuffer[ValueError]
  ): Unit = r.foreach { rr =>
    rr.min.foreach(m =>
      if (WellFormednessTestAccess.cmp(v, m).exists(_ < 0)) e += NumericOutOfRange(path(p), s"below minimum ${nb(m)}")
    );
    rr.max.foreach(m =>
      if (WellFormednessTestAccess.cmp(v, m).exists(_ > 0)) e += NumericOutOfRange(path(p), s"above maximum ${nb(m)}")
    )
  }
  private def nb(b: NumericBound) = b match {
    case NumericBound.Signed(x)    => x.toString; case NumericBound.Unsigned(x) => java.lang.Long.toUnsignedString(x);
    case NumericBound.FloatBits(x) => java.lang.Double.longBitsToDouble(x).toString
  }
  private def fileExt(s: String) = s.split('/').lastOption.flatMap(_.split('.').lastOption).filter(_.nonEmpty)
  private def checkUrl(
    r: UrlRestrictions,
    u: String,
    p: List[ValuePathSegment],
    e: mutable.ListBuffer[ValueError]
  ): Unit = if (u.isEmpty) e += UrlEmpty(path(p))
  else {
    val m = "^([A-Za-z][A-Za-z0-9+.-]*):(?://([^/?#@]*@)?([^/?#]*))?.*$".r;
    u match {
      case m(sch, _, host) =>
        r.allowedSchemes.foreach(a => if (!a.exists(_.equalsIgnoreCase(sch))) e += UrlSchemeNotAllowed(path(p), sch));
        r.allowedHosts.foreach(a =>
          if (host == null || host.isEmpty) e += UrlHostMissing(path(p))
          else if (!a.exists(_.equalsIgnoreCase(host))) e += UrlHostNotAllowed(path(p), host)
        );
      case _ => e += UrlInvalid(path(p), u, "relative URL without a base")
    }
  }
  private def checkQuantity(
    s: QuantitySpec,
    q: QuantityValue,
    p: List[ValuePathSegment],
    e: mutable.ListBuffer[ValueError]
  ): Unit = {
    val ok = if (s.allowedSuffixes.isEmpty) q.unit == s.baseUnit else s.allowedSuffixes.contains(q.unit);
    if (!ok) e += QuantityUnitNotAllowed(path(p), q.unit)
  }
  private def disc(g: SchemaGraph, b: UnionBranch, v: SchemaValue): Boolean = b.discriminator match {
    case DiscriminatorRule.Prefix(x)   => str(v).exists(_.startsWith(x));
    case DiscriminatorRule.Suffix(x)   => str(v).exists(_.endsWith(x));
    case DiscriminatorRule.Contains(x) => str(v).exists(_.contains(x));
    case DiscriminatorRule.Regex(x)    =>
      str(v).exists(s =>
        try Pattern.compile(x).matcher(s).find()
        catch { case _: Exception => false }
      );
    case DiscriminatorRule.FieldEquals(f) =>
      v match {
        case RecordValue(fs) =>
          recFields(g, b.body).flatMap(_.zip(fs).find(_._1 == f.fieldName)).exists { case (_, vv) =>
            f.literal.forall(l => str(vv).contains(l))
          };
        case _ => false
      };
    case DiscriminatorRule.FieldAbsent(n) =>
      v match { case RecordValue(fs) => recFields(g, b.body).exists(!_.contains(n)); case _ => false }
  }
  private def str(v: SchemaValue): Option[String] = v match {
    case StringValue(s) => Some(s); case TextValue(s, _) => Some(s); case UrlValue(s) => Some(s);
    case PathValue(s)   => Some(s); case _               => None
  }
  private def recFields(g: SchemaGraph, t: SchemaType): Option[List[String]] =
    RefResolution.resolveRef(g, t).toOption.collect { case SchemaType(RecordType(fs), _) => fs.map(_.name) }
}
private object WellFormednessTestAccess {
  def cmp(a: NumericBound, b: NumericBound): Option[Int] = (a, b) match {
    case (NumericBound.Signed(x), NumericBound.Signed(y))       => Some(java.lang.Long.compare(x, y));
    case (NumericBound.Unsigned(x), NumericBound.Unsigned(y))   => Some(java.lang.Long.compareUnsigned(x, y));
    case (NumericBound.FloatBits(x), NumericBound.FloatBits(y)) =>
      Some(java.lang.Double.compare(java.lang.Double.longBitsToDouble(x), java.lang.Double.longBitsToDouble(y)));
    case _ => None
  }
}
