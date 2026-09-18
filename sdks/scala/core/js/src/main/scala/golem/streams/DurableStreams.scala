/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 */
package golem.streams

import golem.{HostApi, Uuid}
import golem.config.Secret
import golem.schema.AgentStream
import zio.blocks.schema.Schema
import zio.blocks.schema.json.{Json, JsonCodecDeriver}

/**
 * External Durable Streams clients. Transport, authentication and durability
 * are host-owned.
 */
object DurableStreams {

  /**
   * JSON messages are decoded on demand, including nested arrays and exact
   * integers.
   */
  def json[A: Schema](
    url: String,
    options: DurableStreamReadOptions = DurableStreamReadOptions(),
    auth: Option[Secret[String]] = None
  ): AgentStream[A] = {
    val codec = Schema[A].derive(JsonCodecDeriver)
    DurableStreamReader.create(
      new DurableStreamHostApi(auth),
      url,
      true,
      options,
      bytes =>
        if (bytes.isEmpty) Vector.empty
        else
          Json.parse(bytes.toArray).fold(throw _, identity) match {
            case array: Json.Array => array.elements.toVector
            case _                 => throw DurableStreamError(DurableStreamErrorKind.ProtocolError, "expected a JSON message array")
          },
      (value: Json) => codec.decode(value).fold(throw _, identity)
    )
  }

  /**
   * A byte sequence; HTTP batches and original append boundaries are not
   * messages.
   */
  def bytes(
    url: String,
    options: DurableStreamReadOptions = DurableStreamReadOptions(),
    auth: Option[Secret[String]] = None
  ): AgentStream[Byte] =
    DurableStreamReader.create(
      new DurableStreamHostApi(auth),
      url,
      false,
      options,
      (bytes: Vector[Byte]) => bytes,
      (byte: Byte) => byte
    )

  /**
   * Allocate once using the host's durable identity generator; never fork
   * automatically.
   */
  def newProducer(): DurableStreamProducer =
    DurableStreamProducer(Uuid.toStandardString(HostApi.generateIdempotencyKey()))

  def jsonWriter[A: Schema](
    url: String,
    producer: DurableStreamProducer = newProducer(),
    auth: Option[Secret[String]] = None,
    timeoutMs: Long = 30000,
    retry: DurableStreamRetry = DurableStreamRetry()
  ): DurableStreamWriter[A] = {
    val codec = Schema[A].derive(JsonCodecDeriver)
    new DurableStreamWriter(
      new DurableStreamHostApi(auth),
      url,
      "application/json",
      producer,
      timeoutMs,
      retry,
      values => DurableStreamPayload.Json(values.map(codec.encodeToString))
    )
  }

  def byteWriter(
    url: String,
    contentType: String = "application/octet-stream",
    producer: DurableStreamProducer = newProducer(),
    auth: Option[Secret[String]] = None,
    timeoutMs: Long = 30000,
    retry: DurableStreamRetry = DurableStreamRetry()
  ): DurableStreamWriter[Byte] =
    new DurableStreamWriter(
      new DurableStreamHostApi(auth),
      url,
      contentType,
      producer,
      timeoutMs,
      retry,
      values => DurableStreamPayload.Bytes(values)
    )
}
