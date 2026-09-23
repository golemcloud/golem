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

package golem.schema.wire

import golem.schema.*

import scala.collection.mutable

/** A concrete codec. Neither direction interprets a schema graph. */
trait ConcreteCodec[A] {
  def write(value: A, out: WireValues): Int
  def read(in: WireValuesReader, index: Int): A
  def describe(out: WireTypes): Int

  final def graph: WitSchemaGraph = {
    val out = new WireTypes
    out.finish(describe(out))
  }

  final def encode(value: A): WitTypedSchemaValue = WitTypedSchemaValue(graph, encodeValue(value))

  final def encodeValue(value: A): WitSchemaValueTree = {
    val transaction = new AgentStreamOutputTransaction
    try
      AgentStreamOutputTransaction.capture(transaction) {
        val out  = new WireValues
        val root = write(value, out)
        WitSchemaValueTree(out.nodes.toVector, root)
      }
    catch {
      case error: Throwable =>
        AgentStreamOwnership.cleanup(transaction.rollback())
        throw error
    }
  }

  final def decode(value: WitSchemaValueTree): A = {
    val in = new WireValuesReader(value)
    try {
      val result = read(in, value.root)
      in.finish()
      result
    } catch {
      case error: Throwable =>
        in.abort()
        throw error
    }
  }

  final def xmap[B](from: A => B, to: B => A): ConcreteCodec[B] = {
    val self = this
    new ConcreteCodec[B] {
      def write(value: B, out: WireValues): Int     = self.write(to(value), out)
      def read(in: WireValuesReader, index: Int): B = from(self.read(in, index))
      def describe(out: WireTypes): Int             = self.describe(out)
    }
  }
}

/** One type-index space for all fields of a generated descriptor. */
final class WireTypes {
  private val nodes = mutable.ArrayBuffer.empty[WitSchemaTypeNode]
  private val defs  = mutable.ArrayBuffer.empty[WitSchemaTypeDef]
  private val names = mutable.Map.empty[String, Int]

  def add(body: WitSchemaTypeBody, metadata: MetadataEnvelope = MetadataEnvelope.empty): Int = {
    nodes += WitSchemaTypeNode(body, metadata)
    nodes.length - 1
  }

  def named(id: String, name: String)(body: => Int): Int = {
    val index = names.getOrElseUpdate(
      id, {
        val index = defs.length
        defs += WitSchemaTypeDef(id, Some(name), -1)
        index
      }
    )
    if (defs(index).body == -1) {
      // Reserve before descending so mutually recursive definitions close by index.
      defs(index) = defs(index).copy(body = -2)
      defs(index) = defs(index).copy(body = body)
    }
    add(WitSchemaTypeBody.RefType(index))
  }

  def finish(root: Int): WitSchemaGraph = WitSchemaGraph(nodes.toVector, defs.toVector, root)
}

final class WireValues {
  val nodes: mutable.ArrayBuffer[WitSchemaValueNode] = mutable.ArrayBuffer.empty
  private val resources                              = mutable.Set.empty[Any]

  def add(node: WitSchemaValueNode): Int = {
    WireValuesReader.resource(node).foreach { key =>
      if (!resources.add(key)) throw SchemaEncodeError("owned resource used more than once")
    }
    nodes += node
    nodes.length - 1
  }
}

/**
 * Tracks structure and affine resources without constructing an owned value
 * model.
 */
final class WireValuesReader(val tree: WitSchemaValueTree) {
  private val active    = mutable.Set.empty[Int]
  private val resources = mutable.Set.empty[Any]
  private val claimed   = mutable.Set.empty[Int]
  private val streams   = mutable.Set.empty[GuestSchemaValueStream]

  def at[A](index: Int)(decode: PartialFunction[WitSchemaValueNode, A]): A = {
    if (index < 0 || index >= tree.valueNodes.length) throw SchemaDecodeError(s"invalid value node index $index")
    if (!active.add(index)) throw SchemaDecodeError(s"cyclic value node index $index")
    try {
      val node = tree.valueNodes(index)
      WireValuesReader.resource(node).foreach { key =>
        if (!claimed.add(index) || !resources.add(key)) throw SchemaDecodeError("owned resource used more than once")
      }
      node match {
        case WitSchemaValueNode.StreamValue(handle) => handle.withHandle(streams.add)
        case _                                      => ()
      }
      decode.applyOrElse(node, (_: WitSchemaValueNode) => throw SchemaDecodeError(s"unexpected value node at $index"))
    } finally active.remove(index)
  }

  def finish(): Unit =
    tree.valueNodes.indices.foreach { index =>
      if (!claimed(index) && WireValuesReader.isResource(tree.valueNodes(index)))
        throw SchemaDecodeError(s"unreachable owned resource at $index")
    }

  def abort(): Unit = {
    tree.valueNodes.foreach {
      case WitSchemaValueNode.SecretValue(handle)          => handle.take()
      case WitSchemaValueNode.QuotaTokenHandle(handle)     => handle.take()
      case WitSchemaValueNode.PermissionCardHandle(handle) => handle.take()
      case WitSchemaValueNode.StreamValue(handle)          => handle.take().foreach(streams.add)
      case _                                               => ()
    }
    streams.foreach(stream => AgentStreamOwnership.cleanup(stream.dispose()))
    streams.clear()
  }
}

object WireValuesReader {
  private[wire] def isResource(node: WitSchemaValueNode): Boolean = node match {
    case _: WitSchemaValueNode.SecretValue | _: WitSchemaValueNode.QuotaTokenHandle |
        _: WitSchemaValueNode.PermissionCardHandle | _: WitSchemaValueNode.StreamValue =>
      true
    case _ => false
  }

  private[wire] def resource(node: WitSchemaValueNode): Option[Any] = {
    def present(value: Option[Any]): Option[Any] =
      Some(value.getOrElse(throw SchemaDecodeError("owned resource was already transferred")))
    node match {
      case WitSchemaValueNode.SecretValue(handle)          => present(handle.withHandle(identity))
      case WitSchemaValueNode.QuotaTokenHandle(handle)     => present(handle.withHandle(identity))
      case WitSchemaValueNode.PermissionCardHandle(handle) => present(handle.withHandle(identity))
      case WitSchemaValueNode.StreamValue(handle)          => present(handle.ownershipKey)
      case _                                               => None
    }
  }
}

/** Lazy codec references emitted for recursive concrete Scala types. */
final class ConcreteCodecs {
  private val codecs = mutable.Map.empty[String, ConcreteCodec[?]]

  def ref[A](id: String): ConcreteCodec[A] = codecs(id).asInstanceOf[ConcreteCodec[A]]

  def define[A](id: String)(make: => ConcreteCodec[A]): ConcreteCodec[A] =
    codecs
      .getOrElseUpdate(
        id,
        new ConcreteCodec[A] {
          private lazy val codec                        = make
          def write(value: A, out: WireValues): Int     = codec.write(value, out)
          def read(in: WireValuesReader, index: Int): A = codec.read(in, index)
          def describe(out: WireTypes): Int             = codec.describe(out)
        }
      )
      .asInstanceOf[ConcreteCodec[A]]
}

object ConcreteCodec {
  inline def derived[A]: ConcreteCodec[A] = ${ ConcreteCodecMacro.derive[A] }

  def scalar[A](body: WitSchemaTypeBody)(writeNode: A => WitSchemaValueNode)(
    readNode: PartialFunction[WitSchemaValueNode, A]
  ): ConcreteCodec[A] = new ConcreteCodec[A] {
    def write(value: A, out: WireValues): Int     = out.add(writeNode(value))
    def read(in: WireValuesReader, index: Int): A = in.at(index)(readNode)
    def describe(out: WireTypes): Int             = out.add(body)
  }

  val unit: ConcreteCodec[Unit] =
    scalar[Unit](WitSchemaTypeBody.TupleType(Vector.empty))(_ => WitSchemaValueNode.TupleValue(Vector.empty)) {
      case WitSchemaValueNode.TupleValue(fields) if fields.isEmpty => ()
    }
  val boolean: ConcreteCodec[Boolean] =
    scalar[Boolean](WitSchemaTypeBody.BoolType)(WitSchemaValueNode.BoolValue.apply) {
      case WitSchemaValueNode.BoolValue(value) => value
    }
  val byte: ConcreteCodec[Byte] = scalar[Byte](WitSchemaTypeBody.S8Type())(WitSchemaValueNode.S8Value.apply) {
    case WitSchemaValueNode.S8Value(value) => value
  }
  val short: ConcreteCodec[Short] = scalar[Short](WitSchemaTypeBody.S16Type())(WitSchemaValueNode.S16Value.apply) {
    case WitSchemaValueNode.S16Value(value) => value
  }
  val int: ConcreteCodec[Int] = scalar[Int](WitSchemaTypeBody.S32Type())(WitSchemaValueNode.S32Value.apply) {
    case WitSchemaValueNode.S32Value(value) => value
  }
  val long: ConcreteCodec[Long] = scalar[Long](WitSchemaTypeBody.S64Type())(WitSchemaValueNode.S64Value.apply) {
    case WitSchemaValueNode.S64Value(value) => value
  }
  val float: ConcreteCodec[Float] = scalar[Float](WitSchemaTypeBody.F32Type())(WitSchemaValueNode.F32Value.apply) {
    case WitSchemaValueNode.F32Value(value) => value
  }
  val double: ConcreteCodec[Double] = scalar[Double](WitSchemaTypeBody.F64Type())(WitSchemaValueNode.F64Value.apply) {
    case WitSchemaValueNode.F64Value(value) => value
  }
  val char: ConcreteCodec[Char] =
    scalar[Char](WitSchemaTypeBody.CharType)(value => WitSchemaValueNode.CharValue(value.toInt)) {
      case WitSchemaValueNode.CharValue(value) if value >= 0 && value <= Char.MaxValue.toInt => value.toChar
    }
  val string: ConcreteCodec[String] =
    scalar[String](WitSchemaTypeBody.StringType)(WitSchemaValueNode.StringValue.apply) {
      case WitSchemaValueNode.StringValue(value) => value
    }

  val path: ConcreteCodec[GolemPath] = scalar[GolemPath](
    WitSchemaTypeBody.PathType(PathSpec(PathDirection.InOut, PathKind.Any))
  )(value => WitSchemaValueNode.PathValue(value.value)) { case WitSchemaValueNode.PathValue(value) =>
    new GolemPath(value)
  }
  val url: ConcreteCodec[Url] =
    scalar[Url](WitSchemaTypeBody.UrlType(UrlRestrictions.empty))(value => WitSchemaValueNode.UrlValue(value.value)) {
      case WitSchemaValueNode.UrlValue(value) => new Url(value)
    }
  val ubyte: ConcreteCodec[golem.UByte] = scalar[golem.UByte](WitSchemaTypeBody.U8Type()) { value =>
    require(value.value >= 0 && value.value <= 255); WitSchemaValueNode.U8Value(value.value.toInt)
  } { case WitSchemaValueNode.U8Value(value) if value >= 0 && value <= 255 => new golem.UByte(value.toShort) }
  val ushort: ConcreteCodec[golem.UShort] = scalar[golem.UShort](WitSchemaTypeBody.U16Type()) { value =>
    require(value.value >= 0 && value.value <= 65535); WitSchemaValueNode.U16Value(value.value)
  } { case WitSchemaValueNode.U16Value(value) if value >= 0 && value <= 65535 => new golem.UShort(value) }
  val uint: ConcreteCodec[golem.UInt] = scalar[golem.UInt](WitSchemaTypeBody.U32Type()) { value =>
    require(value.value >= 0 && value.value <= 0xffffffffL); WitSchemaValueNode.U32Value(value.value)
  } { case WitSchemaValueNode.U32Value(value) if value >= 0 && value <= 0xffffffffL => new golem.UInt(value) }
  val ulong: ConcreteCodec[golem.ULong] =
    scalar[golem.ULong](WitSchemaTypeBody.U64Type())(value => WitSchemaValueNode.U64Value(U64.toRawBits(value.value))) {
      case WitSchemaValueNode.U64Value(value) => new golem.ULong(U64.fromRawBits(value))
    }
  val uuid: ConcreteCodec[golem.Uuid] = {
    val bits = scalar[BigInt](WitSchemaTypeBody.U64Type())(value => WitSchemaValueNode.U64Value(U64.toRawBits(value))) {
      case WitSchemaValueNode.U64Value(value) => U64.fromRawBits(value)
    }.asInstanceOf[ConcreteCodec[Any]]
    product[golem.Uuid]("uuid.Uuid", "uuid", Vector("high-bits", "low-bits"), Vector(bits, bits), false) { values =>
      new golem.Uuid(values(0).asInstanceOf[BigInt], values(1).asInstanceOf[BigInt])
    }
  }
  val instant: ConcreteCodec[java.time.Instant] = scalar[java.time.Instant](WitSchemaTypeBody.DatetimeType)(value =>
    WitSchemaValueNode.DatetimeValue(Datetime(value.getEpochSecond, value.getNano))
  ) {
    case WitSchemaValueNode.DatetimeValue(value) if value.nanoseconds >= 0 && value.nanoseconds < 1000000000 =>
      java.time.Instant.ofEpochSecond(value.seconds, value.nanoseconds.toLong)
  }
  val duration: ConcreteCodec[java.time.Duration] = scalar[java.time.Duration](WitSchemaTypeBody.DurationType)(value =>
    WitSchemaValueNode.DurationValue(WitDurationValuePayload(value.toNanos))
  ) { case WitSchemaValueNode.DurationValue(value) => java.time.Duration.ofNanos(value.nanoseconds) }

  def record(fields: Vector[(String, ConcreteCodec[Any])]): ConcreteCodec[Vector[Any]] =
    new ConcreteCodec[Vector[Any]] {
      def write(value: Vector[Any], out: WireValues): Int = {
        if (value.size != fields.size) throw SchemaEncodeError(s"expected ${fields.size} record fields")
        out.add(WitSchemaValueNode.RecordValue(fields.indices.map(i => fields(i)._2.write(value(i), out)).toVector))
      }
      def read(in: WireValuesReader, index: Int): Vector[Any] = in.at(index) {
        case WitSchemaValueNode.RecordValue(values) if values.size == fields.size =>
          fields.indices.map(i => fields(i)._2.read(in, values(i))).toVector
      }
      def describe(out: WireTypes): Int = out.add(WitSchemaTypeBody.RecordType(fields.map { case (name, codec) =>
        WitNamedFieldType(name, codec.describe(out), MetadataEnvelope.empty)
      }))
    }

  def option[A](element: ConcreteCodec[A]): ConcreteCodec[Option[A]] = new ConcreteCodec[Option[A]] {
    def write(value: Option[A], out: WireValues): Int =
      out.add(WitSchemaValueNode.OptionValue(value.map(element.write(_, out))))
    def read(in: WireValuesReader, index: Int): Option[A] = in.at(index) { case WitSchemaValueNode.OptionValue(value) =>
      value.map(element.read(in, _))
    }
    def describe(out: WireTypes): Int = out.add(WitSchemaTypeBody.OptionType(element.describe(out)))
  }

  def list[A](element: ConcreteCodec[A]): ConcreteCodec[List[A]] = new ConcreteCodec[List[A]] {
    def write(value: List[A], out: WireValues): Int =
      out.add(WitSchemaValueNode.ListValue(value.iterator.map(element.write(_, out)).toVector))
    def read(in: WireValuesReader, index: Int): List[A] = in.at(index) { case WitSchemaValueNode.ListValue(values) =>
      values.iterator.map(element.read(in, _)).toList
    }
    def describe(out: WireTypes): Int = out.add(WitSchemaTypeBody.ListType(element.describe(out)))
  }

  def map[K, V](key: ConcreteCodec[K], value: ConcreteCodec[V]): ConcreteCodec[Map[K, V]] =
    new ConcreteCodec[Map[K, V]] {
      def write(values: Map[K, V], out: WireValues): Int = out.add(
        WitSchemaValueNode.MapValue(
          values.iterator.map { case (k, v) => WitMapEntry(key.write(k, out), value.write(v, out)) }.toVector
        )
      )
      def read(in: WireValuesReader, index: Int): Map[K, V] = in.at(index) {
        case WitSchemaValueNode.MapValue(entries) =>
          entries.iterator.map(e => key.read(in, e.key) -> value.read(in, e.value)).toMap
      }
      def describe(out: WireTypes): Int =
        out.add(WitSchemaTypeBody.MapType(WitMapSpec(key.describe(out), value.describe(out))))
    }

  def result[E, A](err: ConcreteCodec[E], ok: ConcreteCodec[A]): ConcreteCodec[Either[E, A]] =
    new ConcreteCodec[Either[E, A]] {
      def write(value: Either[E, A], out: WireValues): Int = out.add(WitSchemaValueNode.ResultValue(value match {
        case Left(value)  => WitResultValuePayload.ErrValue(Some(err.write(value, out)))
        case Right(value) => WitResultValuePayload.OkValue(Some(ok.write(value, out)))
      }))
      def read(in: WireValuesReader, index: Int): Either[E, A] = in.at(index) {
        case WitSchemaValueNode.ResultValue(WitResultValuePayload.ErrValue(Some(value))) => Left(err.read(in, value))
        case WitSchemaValueNode.ResultValue(WitResultValuePayload.OkValue(Some(value)))  => Right(ok.read(in, value))
      }
      def describe(out: WireTypes): Int =
        out.add(WitSchemaTypeBody.ResultType(WitResultSpec(Some(ok.describe(out)), Some(err.describe(out)))))
    }

  def product[A](id: String, name: String, names: Vector[String], fields: Vector[ConcreteCodec[Any]], tuple: Boolean)(
    construct: Vector[Any] => A
  ): ConcreteCodec[A] = new ConcreteCodec[A] {
    def write(value: A, out: WireValues): Int = {
      val values = fields.indices.map(i => fields(i).write(value.asInstanceOf[Product].productElement(i), out)).toVector
      out.add(if (tuple) WitSchemaValueNode.TupleValue(values) else WitSchemaValueNode.RecordValue(values))
    }
    def read(in: WireValuesReader, index: Int): A = in.at(index) {
      case WitSchemaValueNode.TupleValue(values) if tuple && values.length == fields.length =>
        construct(fields.indices.map(i => fields(i).read(in, values(i))).toVector)
      case WitSchemaValueNode.RecordValue(values) if !tuple && values.length == fields.length =>
        construct(fields.indices.map(i => fields(i).read(in, values(i))).toVector)
    }
    def describe(out: WireTypes): Int =
      if (tuple) out.add(WitSchemaTypeBody.TupleType(fields.map(_.describe(out))))
      else
        out.named(id, name) {
          out.add(WitSchemaTypeBody.RecordType(names.zip(fields).map { case (name, codec) =>
            WitNamedFieldType(name, codec.describe(out), MetadataEnvelope.empty)
          }))
        }
  }

  final case class Case[A](name: String, payload: Option[ConcreteCodec[Any]], get: A => Any, make: Any => A)

  def variant[A](id: String, name: String, cases: Vector[Case[A]], ordinal: A => Int): ConcreteCodec[A] =
    new ConcreteCodec[A] {
      private val isEnum                        = cases.forall(_.payload.isEmpty)
      def write(value: A, out: WireValues): Int = {
        val index = ordinal(value)
        val c     = cases(index)
        out.add(
          if (isEnum) WitSchemaValueNode.EnumValue(index)
          else WitSchemaValueNode.VariantValue(WitVariantValuePayload(index, c.payload.map(_.write(c.get(value), out))))
        )
      }
      def read(in: WireValuesReader, index: Int): A = in.at(index) {
        case WitSchemaValueNode.EnumValue(c) if isEnum && c >= 0 && c < cases.length => cases(c).make(())
        case WitSchemaValueNode.VariantValue(WitVariantValuePayload(c, value))
            if !isEnum && c >= 0 && c < cases.length =>
          (cases(c).payload, value) match {
            case (None, None)               => cases(c).make(())
            case (Some(codec), Some(child)) => cases(c).make(codec.read(in, child))
            case _                          => throw SchemaDecodeError("variant payload does not match its case")
          }
      }
      def describe(out: WireTypes): Int = out.named(id, name) {
        out.add(
          if (isEnum) WitSchemaTypeBody.EnumType(cases.map(_.name))
          else
            WitSchemaTypeBody.VariantType(
              cases.map(c => WitVariantCaseType(c.name, c.payload.map(_.describe(out)), MetadataEnvelope.empty))
            )
        )
      }
    }
}
