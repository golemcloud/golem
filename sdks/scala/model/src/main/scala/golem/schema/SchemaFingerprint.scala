/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://license.golem.cloud/LICENSE
 */

package golem.schema

import golem.schema.SchemaTypeBody._
import golem.schema.validation.WellFormedness

import java.nio.charset.StandardCharsets
import scala.collection.immutable.ListMap
import scala.collection.mutable

/** BLAKE3-256 fingerprint of the v1 deterministic-CBOR schema closure. */
final case class SchemaFingerprintV1 private (bytes: Vector[Byte]) {
  require(bytes.length == 32, "a schema fingerprint must contain 32 bytes")
  def toHex: String             = bytes.iterator.map(b => f"${b & 0xff}%02x").mkString
  override def toString: String = toHex
}

sealed trait SchemaFingerprintError extends Product with Serializable { def message: String }
object SchemaFingerprintError {
  final case class InvalidSchema(message: String)                  extends SchemaFingerprintError
  final case class DuplicateSetValue(field: String, value: String) extends SchemaFingerprintError {
    def message: String = s"duplicate value `$value` in set-valued schema field `$field`"
  }
}

object SchemaFingerprintV1 {
  val FormatVersion: Int = 1

  /**
   * Computes the fingerprint for `element`; `None` is the Component Model unit
   * stream element.
   */
  def compute(graph: SchemaGraph, element: Option[SchemaType]): Either[SchemaFingerprintError, SchemaFingerprintV1] =
    canonicalBytes(graph, element).map(bytes => SchemaFingerprintV1(Blake3.hash(bytes).toVector))

  /**
   * Deterministic-CBOR bytes exposed for interoperability tests and
   * diagnostics.
   */
  def canonicalBytes(
    graph: SchemaGraph,
    element: Option[SchemaType]
  ): Either[SchemaFingerprintError, Vector[Byte]] =
    try {
      val root      = element.getOrElse(SchemaType(TupleType(Nil)))
      val reachable = reachableDefinitions(graph, root)
      val projected = SchemaGraph(ListMap.from(reachable), root)
      WellFormedness.validateGraph(projected) match {
        case Left(errors) => Left(SchemaFingerprintError.InvalidSchema(errors.map(_.message).mkString("; ")))
        case Right(_)     =>
          val encoder = new CborEncoder
          encoder.array(4)
          encoder.text("golem-schema-fingerprint")
          encoder.uint(FormatVersion.toLong)
          if (element.isEmpty) { encoder.array(2); encoder.uint(0); encodeMetadata(encoder, MetadataEnvelope.empty) }
          else encodeType(encoder, root)
          encoder.array(reachable.size)
          reachable.foreach { case (id, definition) =>
            encoder.array(3); encoder.text(id); encoder.optionalText(definition.name);
            encodeType(encoder, definition.body)
          }
          Right(encoder.result)
      }
    } catch { case Failure(error) => Left(error) }

  private final case class Failure(error: SchemaFingerprintError) extends RuntimeException

  private def reachableDefinitions(graph: SchemaGraph, root: SchemaType): List[(String, SchemaTypeDef)] = {
    val seen                       = mutable.Set.empty[String]
    def visit(t: SchemaType): Unit = t.body match {
      case RefType(id) if seen.add(id) => graph.defs.get(id).foreach(d => visit(d.body))
      case RecordType(xs)              => xs.foreach(x => visit(x.body))
      case VariantType(xs)             => xs.foreach(_.payload.foreach(visit))
      case TupleType(xs)               => xs.foreach(visit)
      case ListType(x)                 => visit(x)
      case FixedListType(x, _)         => visit(x)
      case MapType(k, v)               => visit(k); visit(v)
      case OptionType(x)               => visit(x)
      case ResultType(ok, err)         => ok.foreach(visit); err.foreach(visit)
      case UnionType(xs)               => xs.foreach(x => visit(x.body))
      case SecretType(x)               => visit(x.inner)
      case FutureType(x)               => x.foreach(visit)
      case StreamType(x)               => x.foreach(visit)
      case _                           => ()
    }
    visit(root)
    graph.defs.iterator.filter(x => seen(x._1)).toList.sortWith((a, b) => utf8Compare(a._1, b._1) < 0)
  }

  private def encodeType(e: CborEncoder, t: SchemaType): Unit = {
    def leaf(tag: Int): Unit                 = { e.array(2); e.uint(tag); encodeMetadata(e, t.metadata) }
    def unary(tag: Int, x: SchemaType): Unit = {
      e.array(3); e.uint(tag); encodeType(e, x); encodeMetadata(e, t.metadata)
    }
    def numeric(tag: Int, r: Option[NumericRestrictions]): Unit = {
      e.array(3); e.uint(tag)
      r.flatMap(_.normalize) match {
        case None    => e.nil()
        case Some(n) =>
          e.array(3); optionalBound(e, n.min); optionalBound(e, n.max); e.optionalText(n.unit.filter(_.nonEmpty))
      }
      encodeMetadata(e, t.metadata)
    }
    t.body match {
      case RefType(id)    => e.array(3); e.uint(1); e.text(id); encodeMetadata(e, t.metadata)
      case BoolType       => leaf(2)
      case S8Type(r)      => numeric(3, r); case S16Type(r) => numeric(4, r); case S32Type(r)  => numeric(5, r)
      case S64Type(r)     => numeric(6, r); case U8Type(r)  => numeric(7, r); case U16Type(r)  => numeric(8, r)
      case U32Type(r)     => numeric(9, r); case U64Type(r) => numeric(10, r); case F32Type(r) => numeric(11, r)
      case F64Type(r)     => numeric(12, r)
      case CharType       => leaf(13); case StringType      => leaf(14)
      case RecordType(xs) =>
        e.array(3); e.uint(15); e.array(xs.size);
        xs.foreach { x =>
          e.array(3); e.text(x.name); encodeType(e, x.body); encodeMetadata(e, x.metadata)
        };
        encodeMetadata(e, t.metadata)
      case VariantType(xs) =>
        e.array(3); e.uint(16); e.array(xs.size);
        xs.foreach { x =>
          e.array(3); e.text(x.name); optionalType(e, x.payload); encodeMetadata(e, x.metadata)
        };
        encodeMetadata(e, t.metadata)
      case EnumType(xs)  => names(e, 17, xs, t.metadata)
      case FlagsType(xs) => names(e, 18, xs, t.metadata)
      case TupleType(xs) =>
        e.array(3); e.uint(19); e.array(xs.size); xs.foreach(encodeType(e, _)); encodeMetadata(e, t.metadata)
      case ListType(x)         => unary(20, x)
      case FixedListType(x, n) =>
        e.array(4); e.uint(21); encodeType(e, x); e.uint(unsignedInt(n)); encodeMetadata(e, t.metadata)
      case MapType(k, v)       => e.array(4); e.uint(22); encodeType(e, k); encodeType(e, v); encodeMetadata(e, t.metadata)
      case OptionType(x)       => unary(23, x)
      case ResultType(ok, err) =>
        e.array(4); e.uint(24); optionalType(e, ok); optionalType(e, err); encodeMetadata(e, t.metadata)
      case TextType(r) =>
        e.array(6); e.uint(25); optionalSet(e, "text.languages", r.languages); optionalUint(e, r.minLength)
        optionalUint(e, r.maxLength); e.optionalText(r.regex); encodeMetadata(e, t.metadata)
      case BinaryType(r) =>
        e.array(5); e.uint(26); optionalSet(e, "binary.mime_types", r.mimeTypes); optionalUint(e, r.minBytes)
        optionalUint(e, r.maxBytes); encodeMetadata(e, t.metadata)
      case PathType(s) =>
        e.array(6); e.uint(27);
        e.uint(s.direction match {
          case PathDirection.Input => 0; case PathDirection.Output => 1; case PathDirection.InOut => 2
        })
        e.uint(s.kind match { case PathKind.File => 0; case PathKind.Directory => 1; case PathKind.Any => 2 })
        optionalSet(e, "path.allowed_mime_types", s.allowedMimeTypes);
        optionalSet(e, "path.allowed_extensions", s.allowedExtensions)
        encodeMetadata(e, t.metadata)
      case UrlType(r) =>
        e.array(4); e.uint(28); optionalSet(e, "url.allowed_schemes", r.allowedSchemes)
        optionalSet(e, "url.allowed_hosts", r.allowedHosts); encodeMetadata(e, t.metadata)
      case DatetimeType    => leaf(29); case DurationType => leaf(30)
      case QuantityType(s) =>
        e.array(6); e.uint(31); e.text(s.baseUnit); e.array(s.allowedSuffixes.size); s.allowedSuffixes.foreach(e.text)
        optionalQuantity(e, s.min); optionalQuantity(e, s.max); encodeMetadata(e, t.metadata)
      case UnionType(xs) =>
        e.array(3); e.uint(32); e.array(xs.size);
        xs.foreach { x =>
          e.array(4); e.text(x.tag); encodeType(e, x.body); discriminator(e, x.discriminator);
          encodeMetadata(e, x.metadata)
        };
        encodeMetadata(e, t.metadata)
      case SecretType(s) =>
        e.array(4); e.uint(33); encodeType(e, s.inner); e.optionalText(s.category); encodeMetadata(e, t.metadata)
      case QuotaTokenType(s)     => e.array(3); e.uint(34); e.optionalText(s.resourceName); encodeMetadata(e, t.metadata)
      case FutureType(x)         => e.array(3); e.uint(35); optionalType(e, x); encodeMetadata(e, t.metadata)
      case StreamType(x)         => e.array(3); e.uint(36); optionalType(e, x); encodeMetadata(e, t.metadata)
      case PermissionCardType(s) => e.array(3); e.uint(37); e.bool(s.polymorphic); encodeMetadata(e, t.metadata)
    }
  }

  private def optionalBound(e: CborEncoder, b: Option[NumericBound]): Unit = b match {
    case None                            => e.nil()
    case Some(NumericBound.Signed(x))    => e.array(2); e.uint(0); e.sint(x)
    case Some(NumericBound.Unsigned(x))  => e.array(2); e.uint(1); e.uint(x)
    case Some(NumericBound.FloatBits(x)) =>
      e.array(2); e.uint(2); e.uint(if (java.lang.Double.longBitsToDouble(x) == 0.0) 0 else x)
  }
  private def optionalType(e: CborEncoder, x: Option[SchemaType]): Unit = x.fold(e.nil())(encodeType(e, _))
  private def optionalUint(e: CborEncoder, x: Option[Int]): Unit        = x.fold(e.nil())(n => e.uint(unsignedInt(n)))
  private def unsignedInt(n: Int): Long                                 =
    if (n < 0) throw Failure(SchemaFingerprintError.InvalidSchema("unsigned schema value is negative")) else n.toLong
  private def names(e: CborEncoder, tag: Int, xs: List[String], m: MetadataEnvelope): Unit = {
    e.array(3); e.uint(tag); e.array(xs.size); xs.foreach(e.text); encodeMetadata(e, m)
  }
  private def optionalQuantity(e: CborEncoder, x: Option[QuantityValue]): Unit = x match {
    case None => e.nil(); case Some(q) => e.array(3); e.sint(q.mantissa); e.sint(q.scale.toLong); e.text(q.unit)
  }
  private def discriminator(e: CborEncoder, d: DiscriminatorRule): Unit = d match {
    case DiscriminatorRule.Prefix(x)      => e.array(2); e.uint(0); e.text(x)
    case DiscriminatorRule.Suffix(x)      => e.array(2); e.uint(1); e.text(x)
    case DiscriminatorRule.Contains(x)    => e.array(2); e.uint(2); e.text(x)
    case DiscriminatorRule.Regex(x)       => e.array(2); e.uint(3); e.text(x)
    case DiscriminatorRule.FieldEquals(x) => e.array(3); e.uint(4); e.text(x.fieldName); e.optionalText(x.literal)
    case DiscriminatorRule.FieldAbsent(x) => e.array(2); e.uint(5); e.text(x)
  }
  private def encodeMetadata(e: CborEncoder, m: MetadataEnvelope): Unit = {
    e.array(5); e.optionalText(m.doc); set(e, "metadata.aliases", m.aliases); e.array(m.examples.size);
    m.examples.foreach(e.text)
    e.optionalText(m.deprecated);
    m.role match {
      case None                          => e.nil()
      case Some(Role.Multimodal)         => e.array(1); e.uint(0)
      case Some(Role.UnstructuredText)   => e.array(1); e.uint(1)
      case Some(Role.UnstructuredBinary) => e.array(1); e.uint(2)
      case Some(Role.Other(x))           => e.array(2); e.uint(3); e.text(x)
    }
  }
  private def optionalSet(e: CborEncoder, field: String, xs: Option[List[String]]): Unit =
    xs.fold(e.nil())(set(e, field, _))
  private def set(e: CborEncoder, field: String, input: List[String]): Unit = {
    val xs = input.sortWith((a, b) => utf8Compare(a, b) < 0)
    xs.sliding(2)
      .find(x => x.size == 2 && x.head == x(1))
      .foreach(x => throw Failure(SchemaFingerprintError.DuplicateSetValue(field, x.head)))
    e.array(xs.size); xs.foreach(e.text)
  }
  private def utf8Compare(a: String, b: String): Int = {
    val x = utf8(a); val y = utf8(b); var i = 0
    while (i < x.length && i < y.length) { val c = (x(i) & 255) - (y(i) & 255); if (c != 0) return c; i += 1 }
    x.length - y.length
  }
  private def utf8(s: String): Array[Byte] = {
    var i = 0
    while (i < s.length) {
      val c = s.charAt(i)
      if (Character.isHighSurrogate(c)) {
        if (i + 1 >= s.length || !Character.isLowSurrogate(s.charAt(i + 1)))
          throw Failure(SchemaFingerprintError.InvalidSchema("invalid UTF-8 text"));
        i += 1
      } else if (Character.isLowSurrogate(c)) throw Failure(SchemaFingerprintError.InvalidSchema("invalid UTF-8 text"))
      i += 1
    }
    s.getBytes(StandardCharsets.UTF_8)
  }

  private final class CborEncoder {
    private val out                                 = mutable.ArrayBuffer.empty[Byte]
    def result: Vector[Byte]                        = out.toVector
    def uint(x: Long): Unit                         = major(0, x)
    def sint(x: Long): Unit                         = if (x >= 0) uint(x) else major(1, ~x)
    def array(n: Int): Unit                         = major(4, n.toLong)
    def text(s: String): Unit                       = { val b = utf8(s); major(3, b.length.toLong); out ++= b }
    def optionalText(x: Option[String]): Unit       = x.fold(nil())(text)
    def bool(x: Boolean): Unit                      = out += (if (x) 0xf5 else 0xf4).toByte
    def nil(): Unit                                 = out += 0xf6.toByte
    private def major(kind: Int, value: Long): Unit = {
      def byte(x: Long): Unit = out += x.toByte
      if (java.lang.Long.compareUnsigned(value, 24) < 0) byte((kind << 5) | value)
      else if (java.lang.Long.compareUnsigned(value, 0x100L) < 0) { byte((kind << 5) | 24); byte(value) }
      else if (java.lang.Long.compareUnsigned(value, 0x10000L) < 0) {
        byte((kind << 5) | 25); byte(value >>> 8); byte(value)
      } else if (java.lang.Long.compareUnsigned(value, 0x100000000L) < 0) {
        byte((kind << 5) | 26); (3 to 0 by -1).foreach(i => byte(value >>> (i * 8)))
      } else { byte((kind << 5) | 27); (7 to 0 by -1).foreach(i => byte(value >>> (i * 8))) }
    }
  }

  private object Blake3 {
    private val Iv =
      Array(0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19)
    private val Perm = Array(2, 6, 3, 10, 7, 0, 4, 13, 1, 11, 12, 5, 9, 14, 15, 8)
    private final case class Output(cv: Array[Int], block: Array[Int], counter: Long, length: Int, flags: Int) {
      def chainingValue: Array[Int] = compress(cv, block, counter, length, flags).take(8)
      def root: Array[Byte]         = wordsToBytes(compress(cv, block, 0, length, flags | 8)).take(32)
    }
    def hash(input: Vector[Byte]): Array[Byte] = {
      val chunks           = math.max(1, (input.length + 1023) / 1024); val stack = mutable.ArrayBuffer.empty[Array[Int]]
      var finalOut: Output = null; var chunk                                      = 0
      while (chunk < chunks) {
        val end    = math.min(input.length, (chunk + 1) * 1024); val bytes = input.slice(chunk * 1024, end).toArray
        val output = chunkOutput(bytes, chunk.toLong)
        if (chunk == chunks - 1) finalOut = output
        else {
          var cv = output.chainingValue; var total = chunk + 1
          while ((total & 1) == 0) { cv = parent(stack.remove(stack.size - 1), cv).chainingValue; total >>>= 1 }
          stack += cv
        }
        chunk += 1
      }
      while (stack.nonEmpty) finalOut = parent(stack.remove(stack.size - 1), finalOut.chainingValue)
      finalOut.root
    }
    private def chunkOutput(bytes: Array[Byte], counter: Long): Output = {
      var cv = Iv.clone(); val blocks = math.max(1, (bytes.length + 63) / 64); var i = 0; var result: Output = null
      while (i < blocks) {
        val start = i * 64; val length = math.min(64, bytes.length - start); val blockBytes = Array.fill[Byte](64)(0)
        if (length > 0) Array.copy(bytes, start, blockBytes, 0, length)
        val flags = (if (i == 0) 1 else 0) | (if (i == blocks - 1) 2 else 0)
        result = Output(cv, bytesToWords(blockBytes), counter, math.max(0, length), flags)
        if (i < blocks - 1) cv = result.chainingValue
        i += 1
      };
      result
    }
    private def parent(left: Array[Int], right: Array[Int])                                                     = Output(Iv.clone(), left ++ right, 0, 64, 4)
    private def compress(cv: Array[Int], block: Array[Int], counter: Long, length: Int, flags: Int): Array[Int] = {
      val v                                                       = cv ++ Iv.take(4) ++ Array(counter.toInt, (counter >>> 32).toInt, length, flags); var m = block.clone();
      var r                                                       = 0
      def g(a: Int, b: Int, c: Int, d: Int, x: Int, y: Int): Unit = {
        v(a) += v(b) + x; v(d) = Integer.rotateRight(v(d) ^ v(a), 16); v(c) += v(d);
        v(b) = Integer.rotateRight(v(b) ^ v(c), 12)
        v(a) += v(b) + y; v(d) = Integer.rotateRight(v(d) ^ v(a), 8); v(c) += v(d);
        v(b) = Integer.rotateRight(v(b) ^ v(c), 7)
      }
      while (r < 7) {
        g(0, 4, 8, 12, m(0), m(1)); g(1, 5, 9, 13, m(2), m(3)); g(2, 6, 10, 14, m(4), m(5));
        g(3, 7, 11, 15, m(6), m(7)); g(0, 5, 10, 15, m(8), m(9)); g(1, 6, 11, 12, m(10), m(11));
        g(2, 7, 8, 13, m(12), m(13)); g(3, 4, 9, 14, m(14), m(15)); m = Perm.map(m); r += 1
      }
      Array.tabulate(16)(i => if (i < 8) v(i) ^ v(i + 8) else v(i) ^ cv(i - 8))
    }
    private def bytesToWords(b: Array[Byte]) = Array.tabulate(16)(i =>
      (b(i * 4) & 255) | ((b(i * 4 + 1) & 255) << 8) | ((b(i * 4 + 2) & 255) << 16) | (b(i * 4 + 3) << 24)
    )
    private def wordsToBytes(w: Array[Int]) =
      w.flatMap(x => Array(x.toByte, (x >>> 8).toByte, (x >>> 16).toByte, (x >>> 24).toByte))
  }
}
