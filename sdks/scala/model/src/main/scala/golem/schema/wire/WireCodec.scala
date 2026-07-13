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

import golem.schema._

import scala.collection.immutable.ListMap
import scala.collection.mutable

// Codecs between the recursive in-memory schema model (golem.schema) and the
// flat WIT carriers (golem.schema.wire). Flattening assigns a type-node /
// value-node index to every node and records definitions in a deterministic
// order (sorted by id). `RefType` bodies flatten to `RefType(defIndex)`.
// Unflattening walks indices back into the recursive form, guarding against
// out-of-range and cyclic indices. Mirrors the TS SDK's `wit.ts`.

private object NumericRestrictionCodec {
  def normalize(restrictions: Option[NumericRestrictions]): Option[NumericRestrictions] =
    restrictions.flatMap(_.normalize)
}

/**
 * Incremental encoder for a single flat [[WitSchemaGraph]] holding several
 * independent root types in one shared `typeNodes` pool. Mirrors the Rust
 * `golem-schema` / TS `GraphEncoder`.
 *
 * Seed the encoder with the agent's merged named definitions, then call
 * [[encodeType]] once per inline root (constructor / method / config),
 * collecting the returned indices, and finally [[finish]] to obtain the graph
 * with a placeholder root. Do NOT encode each agent root via
 * [[SchemaWire.schemaGraphToWit]] and reuse those indices: per-graph node
 * indices are only valid within a single encoding.
 */
final class GraphEncoder(defs: ListMap[String, SchemaTypeDef]) {
  private val typeNodes                      = mutable.ArrayBuffer.empty[WitSchemaTypeNode]
  private val sortedIds: Vector[String]      = defs.keys.toVector.sorted
  private val defIndexById: Map[String, Int] = sortedIds.zipWithIndex.toMap

  // Reserve def slots first (filled below) so `RefType` resolves forward
  // references during body encoding.
  private val witDefs: Array[WitSchemaTypeDef] =
    sortedIds.map(id => WitSchemaTypeDef(id, defs(id).name, -1)).toArray
  sortedIds.zipWithIndex.foreach { case (id, i) =>
    witDefs(i) = witDefs(i).copy(body = encodeType(defs(id).body))
  }

  /**
   * Flatten one (possibly recursive) schema type into the shared pool, return
   * its index.
   */
  def encodeType(st: SchemaType): Int = {
    val body = encodeBody(st.body)
    typeNodes += WitSchemaTypeNode(body, st.metadata)
    typeNodes.length - 1
  }

  private def encodeBody(body: SchemaTypeBody): WitSchemaTypeBody = {
    import SchemaTypeBody._
    val W = WitSchemaTypeBody
    body match {
      case RefType(id) =>
        defIndexById.get(id) match {
          case Some(di) => W.RefType(di)
          case None     => throw SchemaEncodeError(s"schema graph references unknown type id '$id'")
        }
      case BoolType           => W.BoolType
      case S8Type(r)          => W.S8Type(NumericRestrictionCodec.normalize(r))
      case S16Type(r)         => W.S16Type(NumericRestrictionCodec.normalize(r))
      case S32Type(r)         => W.S32Type(NumericRestrictionCodec.normalize(r))
      case S64Type(r)         => W.S64Type(NumericRestrictionCodec.normalize(r))
      case U8Type(r)          => W.U8Type(NumericRestrictionCodec.normalize(r))
      case U16Type(r)         => W.U16Type(NumericRestrictionCodec.normalize(r))
      case U32Type(r)         => W.U32Type(NumericRestrictionCodec.normalize(r))
      case U64Type(r)         => W.U64Type(NumericRestrictionCodec.normalize(r))
      case F32Type(r)         => W.F32Type(NumericRestrictionCodec.normalize(r))
      case F64Type(r)         => W.F64Type(NumericRestrictionCodec.normalize(r))
      case CharType           => W.CharType
      case StringType         => W.StringType
      case RecordType(fields) =>
        W.RecordType(fields.map(f => WitNamedFieldType(f.name, encodeType(f.body), f.metadata)).toVector)
      case VariantType(cases) =>
        W.VariantType(cases.map(c => WitVariantCaseType(c.name, c.payload.map(encodeType), c.metadata)).toVector)
      case EnumType(cases)                => W.EnumType(cases.toVector)
      case FlagsType(names)               => W.FlagsType(names.toVector)
      case TupleType(elements)            => W.TupleType(elements.map(encodeType).toVector)
      case ListType(element)              => W.ListType(encodeType(element))
      case FixedListType(element, length) => W.FixedListType(WitFixedListSpec(encodeType(element), length))
      case MapType(key, value)            => W.MapType(WitMapSpec(encodeType(key), encodeType(value)))
      case OptionType(element)            => W.OptionType(encodeType(element))
      case ResultType(ok, err)            => W.ResultType(WitResultSpec(ok.map(encodeType), err.map(encodeType)))
      case TextType(r)                    => W.TextType(r)
      case BinaryType(r)                  => W.BinaryType(r)
      case PathType(s)                    => W.PathType(s)
      case UrlType(r)                     => W.UrlType(r)
      case DatetimeType                   => W.DatetimeType
      case DurationType                   => W.DurationType
      case QuantityType(s)                => W.QuantityType(s)
      case UnionType(branches)            =>
        W.UnionType(
          WitUnionSpec(
            branches.map(br => WitUnionBranch(br.tag, encodeType(br.body), br.discriminator, br.metadata)).toVector
          )
        )
      case SecretType(s)     => W.SecretType(WitSecretSpec(encodeType(s.inner), s.category))
      case QuotaTokenType(s) => W.QuotaTokenType(s)
      case FutureType(e)     => W.FutureType(e.map(encodeType))
      case StreamType(e)     => W.StreamType(e.map(encodeType))
    }
  }

  /** Encode `root` as the graph root and return the finished flat graph. */
  def encodeGraphRoot(root: SchemaType): WitSchemaGraph = {
    val rootIdx = encodeType(root)
    WitSchemaGraph(typeNodes.toVector, witDefs.toVector, rootIdx)
  }

  /**
   * Finish the graph with a placeholder empty-record root. Use after collecting
   * the real root indices via [[encodeType]].
   */
  def finish(): WitSchemaGraph = {
    val rootIdx = encodeType(SchemaType(SchemaTypeBody.RecordType(Nil)))
    WitSchemaGraph(typeNodes.toVector, witDefs.toVector, rootIdx)
  }
}

object SchemaWire {

  // ----------------------------------------------------------------
  // Schema type / graph
  // ----------------------------------------------------------------

  def schemaGraphToWit(graph: SchemaGraph): WitSchemaGraph =
    new GraphEncoder(graph.defs).encodeGraphRoot(graph.root)

  def schemaGraphFromWit(wit: WitSchemaGraph): SchemaGraph = {
    val nodes   = wit.typeNodes
    val witDefs = wit.defs
    val onPath  = Array.fill(nodes.length)(false)

    def idByDefIndex(di: Int): String =
      if (di < 0 || di >= witDefs.length)
        throw SchemaDecodeError(s"def index out of range: $di (defs: ${witDefs.length})")
      else witDefs(di).id

    def fromType(idx: Int): SchemaType = {
      if (idx < 0 || idx >= nodes.length)
        throw SchemaDecodeError(s"type node index out of range: $idx (nodes: ${nodes.length})")
      if (onPath(idx)) throw SchemaDecodeError(s"cyclic type node reference at index $idx")
      onPath(idx) = true
      val node   = nodes(idx)
      val result = SchemaType(fromBody(node.body), node.metadata)
      onPath(idx) = false
      result
    }

    def fromBody(body: WitSchemaTypeBody): SchemaTypeBody = {
      val S = SchemaTypeBody
      body match {
        case WitSchemaTypeBody.RefType(di)        => S.RefType(idByDefIndex(di))
        case WitSchemaTypeBody.BoolType           => S.BoolType
        case WitSchemaTypeBody.S8Type(r)          => S.S8Type(NumericRestrictionCodec.normalize(r))
        case WitSchemaTypeBody.S16Type(r)         => S.S16Type(NumericRestrictionCodec.normalize(r))
        case WitSchemaTypeBody.S32Type(r)         => S.S32Type(NumericRestrictionCodec.normalize(r))
        case WitSchemaTypeBody.S64Type(r)         => S.S64Type(NumericRestrictionCodec.normalize(r))
        case WitSchemaTypeBody.U8Type(r)          => S.U8Type(NumericRestrictionCodec.normalize(r))
        case WitSchemaTypeBody.U16Type(r)         => S.U16Type(NumericRestrictionCodec.normalize(r))
        case WitSchemaTypeBody.U32Type(r)         => S.U32Type(NumericRestrictionCodec.normalize(r))
        case WitSchemaTypeBody.U64Type(r)         => S.U64Type(NumericRestrictionCodec.normalize(r))
        case WitSchemaTypeBody.F32Type(r)         => S.F32Type(NumericRestrictionCodec.normalize(r))
        case WitSchemaTypeBody.F64Type(r)         => S.F64Type(NumericRestrictionCodec.normalize(r))
        case WitSchemaTypeBody.CharType           => S.CharType
        case WitSchemaTypeBody.StringType         => S.StringType
        case WitSchemaTypeBody.RecordType(fields) =>
          S.RecordType(fields.map(f => NamedFieldType(f.name, fromType(f.body), f.metadata)).toList)
        case WitSchemaTypeBody.VariantType(cases) =>
          S.VariantType(cases.map(c => VariantCaseType(c.name, c.payload.map(fromType), c.metadata)).toList)
        case WitSchemaTypeBody.EnumType(cases)     => S.EnumType(cases.toList)
        case WitSchemaTypeBody.FlagsType(names)    => S.FlagsType(names.toList)
        case WitSchemaTypeBody.TupleType(elements) => S.TupleType(elements.map(fromType).toList)
        case WitSchemaTypeBody.ListType(element)   => S.ListType(fromType(element))
        case WitSchemaTypeBody.FixedListType(spec) => S.FixedListType(fromType(spec.element), spec.length)
        case WitSchemaTypeBody.MapType(spec)       => S.MapType(fromType(spec.key), fromType(spec.value))
        case WitSchemaTypeBody.OptionType(element) => S.OptionType(fromType(element))
        case WitSchemaTypeBody.ResultType(spec)    => S.ResultType(spec.ok.map(fromType), spec.err.map(fromType))
        case WitSchemaTypeBody.TextType(r)         => S.TextType(r)
        case WitSchemaTypeBody.BinaryType(r)       => S.BinaryType(r)
        case WitSchemaTypeBody.PathType(sp)        => S.PathType(sp)
        case WitSchemaTypeBody.UrlType(r)          => S.UrlType(r)
        case WitSchemaTypeBody.DatetimeType        => S.DatetimeType
        case WitSchemaTypeBody.DurationType        => S.DurationType
        case WitSchemaTypeBody.QuantityType(sp)    => S.QuantityType(sp)
        case WitSchemaTypeBody.UnionType(spec)     =>
          S.UnionType(
            spec.branches.map(br => UnionBranch(br.tag, fromType(br.body), br.discriminator, br.metadata)).toList
          )
        case WitSchemaTypeBody.SecretType(sp)     => S.SecretType(SecretSpec(fromType(sp.inner), sp.category))
        case WitSchemaTypeBody.QuotaTokenType(sp) => S.QuotaTokenType(sp)
        case WitSchemaTypeBody.FutureType(e)      => S.FutureType(e.map(fromType))
        case WitSchemaTypeBody.StreamType(e)      => S.StreamType(e.map(fromType))
      }
    }

    val defsBuilder = ListMap.newBuilder[String, SchemaTypeDef]
    val seen        = mutable.Set.empty[String]
    witDefs.foreach { d =>
      if (seen.contains(d.id)) throw SchemaDecodeError(s"duplicate def id '${d.id}' in schema graph")
      seen += d.id
      defsBuilder += (d.id -> SchemaTypeDef(fromType(d.body), d.name))
    }
    val root = fromType(wit.root)
    SchemaGraph(defsBuilder.result(), root)
  }

  // ----------------------------------------------------------------
  // Schema value
  // ----------------------------------------------------------------

  /**
   * Verify, before any owned handle is moved, that every [[SchemaValue]]
   * `quota-token` in `value` is still present and that no handle appears more
   * than once. Running this as a preflight keeps lowering atomic: a value tree
   * with an aliased or already-transferred token is rejected without partially
   * transferring any handle. Mirrors the TS SDK's `preflightQuotaHandles`.
   */
  private def preflightCapabilityHandles(value: SchemaValue): Unit = {
    val seenSecrets                 = mutable.Set.empty[GuestSecretHandle]
    val seenQuotaTokens             = mutable.Set.empty[GuestQuotaTokenHandle]
    def visit(v: SchemaValue): Unit = {
      import SchemaValue._
      v match {
        case SecretValue(h) =>
          if (!h.isPresent)
            throw SchemaEncodeError("secret handle was already transferred; an owned secret can only be sent once")
          if (!seenSecrets.add(h))
            throw SchemaEncodeError("the same secret handle appeared more than once in one value tree")
        case QuotaTokenHandle(h) =>
          if (!h.isPresent)
            throw SchemaEncodeError(
              "quota-token handle was already transferred; an owned quota-token can only be sent once"
            )
          if (!seenQuotaTokens.add(h))
            throw SchemaEncodeError("the same quota-token handle appeared more than once in one value tree")
        case RecordValue(fields)  => fields.foreach(visit)
        case VariantValue(_, p)   => p.foreach(visit)
        case TupleValue(elements) => elements.foreach(visit)
        case ListValue(elements)  => elements.foreach(visit)
        case FixedListValue(es)   => es.foreach(visit)
        case MapValue(entries)    => entries.foreach { e => visit(e.key); visit(e.value) }
        case OptionValue(o)       => o.foreach(visit)
        case ResultValue(result)  =>
          result match {
            case SchemaResult.Ok(o)  => o.foreach(visit)
            case SchemaResult.Err(o) => o.foreach(visit)
          }
        case UnionValue(_, body) => visit(body)
        case _                   => ()
      }
    }
    visit(value)
  }

  def schemaValueToWit(value: SchemaValue): WitSchemaValueTree = {
    preflightCapabilityHandles(value)
    val valueNodes = mutable.ArrayBuffer.empty[WitSchemaValueNode]

    def emit(value: SchemaValue): Int = {
      valueNodes += emitNode(value)
      valueNodes.length - 1
    }

    def emitNode(value: SchemaValue): WitSchemaValueNode = {
      import SchemaValue._
      val W = WitSchemaValueNode
      value match {
        case BoolValue(x)                     => W.BoolValue(x)
        case S8Value(x)                       => W.S8Value(x)
        case S16Value(x)                      => W.S16Value(x)
        case S32Value(x)                      => W.S32Value(x)
        case S64Value(x)                      => W.S64Value(x)
        case U8Value(x)                       => W.U8Value(x)
        case U16Value(x)                      => W.U16Value(x)
        case U32Value(x)                      => W.U32Value(x)
        case U64Value(x)                      => W.U64Value(x)
        case F32Value(x)                      => W.F32Value(x)
        case F64Value(x)                      => W.F64Value(x)
        case CharValue(x)                     => W.CharValue(x)
        case StringValue(x)                   => W.StringValue(x)
        case RecordValue(fields)              => W.RecordValue(fields.map(emit).toVector)
        case VariantValue(caseIndex, payload) =>
          W.VariantValue(WitVariantValuePayload(caseIndex, payload.map(emit)))
        case EnumValue(caseIndex)     => W.EnumValue(caseIndex)
        case FlagsValue(flags)        => W.FlagsValue(flags.toVector)
        case TupleValue(elements)     => W.TupleValue(elements.map(emit).toVector)
        case ListValue(elements)      => W.ListValue(elements.map(emit).toVector)
        case FixedListValue(elements) => W.FixedListValue(elements.map(emit).toVector)
        case MapValue(entries)        => W.MapValue(entries.map(e => WitMapEntry(emit(e.key), emit(e.value))).toVector)
        case OptionValue(v)           => W.OptionValue(v.map(emit))
        case ResultValue(result)      =>
          val payload = result match {
            case SchemaResult.Ok(v)  => WitResultValuePayload.OkValue(v.map(emit))
            case SchemaResult.Err(v) => WitResultValuePayload.ErrValue(v.map(emit))
          }
          W.ResultValue(payload)
        case TextValue(text, language)    => W.TextValue(WitTextValuePayload(text, language))
        case BinaryValue(bytes, mimeType) => W.BinaryValue(WitBinaryValuePayload(bytes, mimeType))
        case PathValue(x)                 => W.PathValue(x)
        case UrlValue(x)                  => W.UrlValue(x)
        case DatetimeValue(x)             => W.DatetimeValue(x)
        case DurationValue(nanoseconds)   => W.DurationValue(WitDurationValuePayload(nanoseconds))
        case QuantityValueNode(x)         => W.QuantityValueNode(x)
        case UnionValue(unionTag, body)   => W.UnionValue(WitUnionValuePayload(unionTag, emit(body)))
        case SecretValue(h)               => W.SecretValue(h)
        case QuotaTokenHandle(h)          => W.QuotaTokenHandle(h)
      }
    }

    val root = emit(value)
    WitSchemaValueTree(valueNodes.toVector, root)
  }

  def schemaValueFromWit(wit: WitSchemaValueTree): SchemaValue = {
    val nodes  = wit.valueNodes
    val onPath = Array.fill(nodes.length)(false)
    // An owned `quota-token` handle node may be lifted into the value tree at
    // most once; track which handle nodes have already been claimed so a
    // malformed tree that references the same handle node twice cannot wrap one
    // owned resource into two handles.
    val liftedHandle  = Array.fill(nodes.length)(false)
    val seenRawSecret = mutable.Set.empty[Any]
    val seenRawQuota  = mutable.Set.empty[Any]

    def fromIdx(idx: Int): SchemaValue = {
      if (idx < 0 || idx >= nodes.length)
        throw SchemaDecodeError(s"value node index out of range: $idx (nodes: ${nodes.length})")
      if (onPath(idx)) throw SchemaDecodeError(s"cyclic value node reference at index $idx")
      nodes(idx) match {
        case WitSchemaValueNode.SecretValue(h) =>
          if (liftedHandle(idx))
            throw SchemaDecodeError(s"secret handle node referenced more than once at index $idx")
          val raw = h
            .withHandle(identity)
            .getOrElse(throw SchemaDecodeError(s"secret handle node already consumed at index $idx"))
          if (!seenRawSecret.add(raw))
            throw SchemaDecodeError(s"secret handle resource referenced more than once at index $idx")
          liftedHandle(idx) = true
        case WitSchemaValueNode.QuotaTokenHandle(h) =>
          if (liftedHandle(idx))
            throw SchemaDecodeError(s"quota-token handle node referenced more than once at index $idx")
          val raw = h
            .withHandle(identity)
            .getOrElse(throw SchemaDecodeError(s"quota-token handle node already consumed at index $idx"))
          if (!seenRawQuota.add(raw))
            throw SchemaDecodeError(s"quota-token handle resource referenced more than once at index $idx")
          liftedHandle(idx) = true
        case _ => ()
      }
      onPath(idx) = true
      val result = fromNode(nodes(idx))
      onPath(idx) = false
      result
    }

    // Empty owned `quota-token` handles still present in `nodes`, returning the
    // first such index. With `includeLifted = false` only handle nodes never
    // lifted into the value tree are drained (used to detect a tree that carries
    // handles unreachable from the root); with `includeLifted = true` every
    // handle is drained, including ones already lifted into a partial or rejected
    // value, so no decode failure leaves a live owned resource dangling.
    def drainHandles(includeLifted: Boolean): Option[Int] = {
      var leftover: Option[Int] = None
      var i                     = 0
      while (i < nodes.length) {
        nodes(i) match {
          case WitSchemaValueNode.SecretValue(h) if includeLifted || !liftedHandle(i) =>
            h.take()
            if (leftover.isEmpty) leftover = Some(i)
          case WitSchemaValueNode.QuotaTokenHandle(h) if includeLifted || !liftedHandle(i) =>
            h.take()
            if (leftover.isEmpty) leftover = Some(i)
          case _ => ()
        }
        i += 1
      }
      leftover
    }

    def fromNode(node: WitSchemaValueNode): SchemaValue = {
      val S = SchemaValue
      node match {
        case WitSchemaValueNode.BoolValue(x)             => S.BoolValue(x)
        case WitSchemaValueNode.S8Value(x)               => S.S8Value(x)
        case WitSchemaValueNode.S16Value(x)              => S.S16Value(x)
        case WitSchemaValueNode.S32Value(x)              => S.S32Value(x)
        case WitSchemaValueNode.S64Value(x)              => S.S64Value(x)
        case WitSchemaValueNode.U8Value(x)               => S.U8Value(x)
        case WitSchemaValueNode.U16Value(x)              => S.U16Value(x)
        case WitSchemaValueNode.U32Value(x)              => S.U32Value(x)
        case WitSchemaValueNode.U64Value(x)              => S.U64Value(x)
        case WitSchemaValueNode.F32Value(x)              => S.F32Value(x)
        case WitSchemaValueNode.F64Value(x)              => S.F64Value(x)
        case WitSchemaValueNode.CharValue(x)             => S.CharValue(x)
        case WitSchemaValueNode.StringValue(x)           => S.StringValue(x)
        case WitSchemaValueNode.RecordValue(fields)      => S.RecordValue(fields.map(fromIdx).toList)
        case WitSchemaValueNode.VariantValue(p)          => S.VariantValue(p.caseIndex, p.payload.map(fromIdx))
        case WitSchemaValueNode.EnumValue(caseIndex)     => S.EnumValue(caseIndex)
        case WitSchemaValueNode.FlagsValue(flags)        => S.FlagsValue(flags.toList)
        case WitSchemaValueNode.TupleValue(elements)     => S.TupleValue(elements.map(fromIdx).toList)
        case WitSchemaValueNode.ListValue(elements)      => S.ListValue(elements.map(fromIdx).toList)
        case WitSchemaValueNode.FixedListValue(elements) => S.FixedListValue(elements.map(fromIdx).toList)
        case WitSchemaValueNode.MapValue(entries)        =>
          S.MapValue(entries.map(e => SchemaMapEntry(fromIdx(e.key), fromIdx(e.value))).toList)
        case WitSchemaValueNode.OptionValue(v)       => S.OptionValue(v.map(fromIdx))
        case WitSchemaValueNode.ResultValue(payload) =>
          val r = payload match {
            case WitResultValuePayload.OkValue(v)  => SchemaResult.Ok(v.map(fromIdx))
            case WitResultValuePayload.ErrValue(v) => SchemaResult.Err(v.map(fromIdx))
          }
          S.ResultValue(r)
        case WitSchemaValueNode.TextValue(p)         => S.TextValue(p.text, p.language)
        case WitSchemaValueNode.BinaryValue(p)       => S.BinaryValue(p.bytes, p.mimeType)
        case WitSchemaValueNode.PathValue(x)         => S.PathValue(x)
        case WitSchemaValueNode.UrlValue(x)          => S.UrlValue(x)
        case WitSchemaValueNode.DatetimeValue(x)     => S.DatetimeValue(x)
        case WitSchemaValueNode.DurationValue(p)     => S.DurationValue(p.nanoseconds)
        case WitSchemaValueNode.QuantityValueNode(x) => S.QuantityValueNode(x)
        case WitSchemaValueNode.UnionValue(p)        => S.UnionValue(p.tag, fromIdx(p.body))
        case WitSchemaValueNode.SecretValue(h)       => S.SecretValue(h)
        case WitSchemaValueNode.QuotaTokenHandle(h)  => S.QuotaTokenHandle(h)
      }
    }

    val result =
      try fromIdx(wit.root)
      catch {
        case e: Throwable =>
          // Release every owned `quota-token` handle in the wire tree, including
          // ones already lifted into the partial (now discarded) value, so a
          // failed decode never leaves a live owned resource dangling.
          drainHandles(includeLifted = true)
          throw e
      }

    // A valid tree references every owned `quota-token` handle exactly once from
    // the root. If any handle node was never lifted it was unreachable from the
    // root: the whole decode is rejected, so every handle is released, including
    // those already lifted into `result`.
    drainHandles(includeLifted = false) match {
      case Some(i) =>
        drainHandles(includeLifted = true)
        throw SchemaDecodeError(s"capability handle node not referenced from the root at index $i")
      case None => result
    }
  }

  // ----------------------------------------------------------------
  // Typed schema value
  // ----------------------------------------------------------------

  def typedSchemaValueToWit(tv: TypedSchemaValue): WitTypedSchemaValue =
    WitTypedSchemaValue(schemaGraphToWit(tv.graph), schemaValueToWit(tv.value))

  def typedSchemaValueFromWit(wit: WitTypedSchemaValue): TypedSchemaValue =
    TypedSchemaValue(schemaGraphFromWit(wit.graph), schemaValueFromWit(wit.value))
}
