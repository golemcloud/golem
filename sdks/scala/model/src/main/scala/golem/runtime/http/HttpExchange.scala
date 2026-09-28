// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
package golem.runtime.http

import golem.UShort
import golem.schema._
import golem.schema.SchemaValue._
import scala.concurrent.{ExecutionContext, Future}

/** Headers are ordered occurrences with opaque byte values, never a map. */
final case class HttpHeader(name: String, value: Array[Byte])
object HttpHeader {
  def ascii(name: String, value: String): HttpHeader = {
    require(value.forall(_ <= 127), "header text must be ASCII; use bytes for opaque values")
    HttpHeader(name, value.map(_.toByte).toArray)
  }
}

final case class HttpRequestHead(
  method: String,
  scheme: String,
  authority: String,
  path: String,
  query: Option[String],
  headers: List[HttpHeader]
) {
  def withBody(body: AgentStream[Array[Byte]]): HttpRequest =
    HttpRequest(method, scheme, authority, path, query, headers, body)
}

final case class HttpResponseHead(status: UShort, headers: List[HttpHeader]) {
  def withBody(body: AgentStream[Array[Byte]]): HttpResponse = HttpResponse(status, headers, body)
}

/**
 * The original public path and opaque query are retained, including an empty
 * query.
 */
final case class HttpRequest(
  method: String,
  scheme: String,
  authority: String,
  path: String,
  query: Option[String],
  headers: List[HttpHeader],
  body: AgentStream[Array[Byte]]
) {
  def head: HttpRequestHead = HttpRequestHead(method, scheme, authority, path, query, headers)
}

final case class HttpResponse(status: UShort, headers: List[HttpHeader], body: AgentStream[Array[Byte]]) {
  def head: HttpResponseHead = HttpResponseHead(status, headers)
}

object HttpRequest {
  implicit val intoSchema: IntoSchema[HttpRequest] = new IntoSchema[HttpRequest] {
    val graph: SchemaGraph                       = HttpExchangeCodec.requestGraph
    def toValue(value: HttpRequest): SchemaValue = RecordValue(
      List(
        StringValue(value.method),
        StringValue(value.scheme),
        StringValue(value.authority),
        StringValue(value.path),
        OptionValue(value.query.map(StringValue(_))),
        HttpExchangeCodec.headers(value.headers),
        HttpExchangeCodec.streamInto.toValue(value.body)
      )
    )
  }
  implicit val fromSchema: FromSchema[HttpRequest] = new FromSchema[HttpRequest] {
    def fromValue(value: SchemaValue): Either[FromSchemaError, HttpRequest] = value match {
      case RecordValue(
            List(
              StringValue(method),
              StringValue(scheme),
              StringValue(authority),
              StringValue(path),
              query,
              headers,
              body
            )
          ) =>
        for {
          q <- query match {
                 case OptionValue(None)                 => Right(None)
                 case OptionValue(Some(StringValue(s))) => Right(Some(s))
                 case _                                 => Left(FromSchemaError("invalid HTTP query"))
               }
          h <- HttpExchangeCodec.readHeaders(headers)
          b <- HttpExchangeCodec.streamFrom.fromValue(body)
        } yield HttpRequest(method, scheme, authority, path, q, h, b)
      case _ => Left(FromSchemaError("invalid HTTP request envelope"))
    }
  }
}

object HttpResponse {
  implicit val intoSchema: IntoSchema[HttpResponse] = new IntoSchema[HttpResponse] {
    val graph: SchemaGraph                        = HttpExchangeCodec.responseGraph
    def toValue(value: HttpResponse): SchemaValue = {
      require(value.status.value >= 0 && value.status.value <= 65535, "HTTP status is outside u16")
      RecordValue(
        List(
          U16Value(value.status.value),
          HttpExchangeCodec.headers(value.headers),
          HttpExchangeCodec.streamInto.toValue(value.body)
        )
      )
    }
  }
  implicit val fromSchema: FromSchema[HttpResponse] = new FromSchema[HttpResponse] {
    def fromValue(value: SchemaValue): Either[FromSchemaError, HttpResponse] = value match {
      case RecordValue(List(U16Value(status), headers, body)) if status >= 0 && status <= 65535 =>
        for {
          h <- HttpExchangeCodec.readHeaders(headers)
          b <- HttpExchangeCodec.streamFrom.fromValue(body)
        } yield HttpResponse(UShort(status), h, b)
      case _ => Left(FromSchemaError("invalid HTTP response envelope"))
    }
  }
}

/**
 * Functional middleware runs inside the handler, not around host-served files
 * or OpenAPI.
 */
object HttpHandler {
  type Handler    = HttpRequest => Future[HttpResponse]
  type Middleware = Handler => Handler

  /**
   * First declared middleware is outermost. Stream ownership stays with the
   * response consumer.
   */
  def withMiddleware(handler: Handler, middleware: List[Middleware]): Handler =
    middleware.foldRight(handler)((wrap, next) => wrap(next))

  /**
   * Runs cleanup when the response stream terminates or is disposed, or when
   * obtaining the head fails.
   */
  def ensuring(handler: Handler)(cleanup: () => Future[Unit])(implicit ec: ExecutionContext): Handler = request => {
    lazy val finalized = cleanup()
    val response       =
      try handler(request)
      catch { case scala.util.control.NonFatal(error) => Future.failed(error) }
    response.transformWith {
      case scala.util.Failure(error) => finalized.transformWith(_ => Future.failed(error))
      case scala.util.Success(value) =>
        Future.successful(value.copy(body = value.body.ensuring(() => finalized)))
    }
  }
}

private[http] object HttpExchangeCodec {
  private val bytesType         = t.list(t.u8)
  private val headerType        = t.record(List(NamedFieldType("name", t.string), NamedFieldType("value", bytesType)))
  private val bodyType          = SchemaType(SchemaTypeBody.StreamType(Some(bytesType)))
  val requestGraph: SchemaGraph = SchemaBuilder.graphOf(_ =>
    t.record(
      List(
        NamedFieldType("method", t.string),
        NamedFieldType("scheme", t.string),
        NamedFieldType("authority", t.string),
        NamedFieldType("path", t.string),
        NamedFieldType("query", t.option(t.string)),
        NamedFieldType("headers", t.list(headerType)),
        NamedFieldType("body", bodyType)
      )
    )
  )
  val responseGraph: SchemaGraph = SchemaBuilder.graphOf(_ =>
    t.record(
      List(
        NamedFieldType("status", t.u16),
        NamedFieldType("headers", t.list(headerType)),
        NamedFieldType("body", bodyType)
      )
    )
  )
  private val bytesInto: IntoSchema[Array[Byte]] = new IntoSchema[Array[Byte]] {
    val graph: SchemaGraph                       = SchemaBuilder.graphOf(_ => bytesType)
    def toValue(bytes: Array[Byte]): SchemaValue = ListValue(bytes.iterator.map(b => U8Value(b & 255)).toList)
  }
  private val bytesFrom: FromSchema[Array[Byte]] = new FromSchema[Array[Byte]] {
    def fromValue(value: SchemaValue): Either[FromSchemaError, Array[Byte]] = value match {
      case ListValue(values) =>
        values
          .foldRight[Either[FromSchemaError, List[Byte]]](Right(Nil)) {
            case (U8Value(b), tail) if b >= 0 && b <= 255 => tail.map(b.toByte :: _)
            case _                                        => Left(FromSchemaError("invalid HTTP byte"))
          }
          .map(_.toArray)
      case _ => Left(FromSchemaError("invalid HTTP byte list"))
    }
  }
  val streamInto: IntoSchema[AgentStream[Array[Byte]]] = AgentStream.intoSchema(bytesInto)
  val streamFrom: FromSchema[AgentStream[Array[Byte]]] = AgentStream.fromSchema(bytesFrom)

  def headers(values: List[HttpHeader]): SchemaValue = ListValue(
    values.map(h => RecordValue(List(StringValue(h.name), bytesInto.toValue(h.value))))
  )

  def readHeaders(value: SchemaValue): Either[FromSchemaError, List[HttpHeader]] = value match {
    case ListValue(values) =>
      values.foldRight[Either[FromSchemaError, List[HttpHeader]]](Right(Nil)) {
        case (RecordValue(List(StringValue(name), bytes)), tail) =>
          for { b <- bytesFrom.fromValue(bytes); rest <- tail } yield HttpHeader(name, b) :: rest
        case _ => Left(FromSchemaError("invalid HTTP header"))
      }
    case _ => Left(FromSchemaError("invalid HTTP headers"))
  }
}
