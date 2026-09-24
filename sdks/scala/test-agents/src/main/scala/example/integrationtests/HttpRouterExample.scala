// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
package example.integrationtests

import golem.{BaseAgent, UShort}
import golem.config.{AgentConfig, Config}
import golem.runtime.annotations.*
import golem.runtime.http.*
import golem.schema.AgentStream
import scala.concurrent.Future
import scala.scalajs.js
import scala.scalajs.js.annotation.JSImport
import scala.scalajs.concurrent.JSExecutionContext.Implicits.queue
import zio.blocks.schema.Schema

final case class WebsiteConfig(greeting: String) derives Schema

@httpRouter(
  typeName = "ScalaWebsite",
  mount = "/scala",
  staticBindings = Array(
    ("/", "/site/index.html"),
    ("/assets/*", "/missing/$1"),
    ("/assets/*", "/site/$1")
  ),
  cors = Array("*")
)
trait ScalaWebsite extends BaseAgent with AgentConfig[WebsiteConfig] {
  @httpHandler def serve(request: HttpRequest, principal: golem.Principal): Future[HttpResponse]
  @openApiProvider def describe(): Future[String]
}

@agentImplementation()
final class ScalaWebsiteImpl(config: Config[WebsiteConfig]) extends ScalaWebsite {
  private def text(value: String): HttpResponse = {
    var remaining: Option[Array[Byte]] = Some(value.getBytes("UTF-8"))
    HttpResponse(
      UShort(200),
      List(HttpHeader.ascii("content-type", "text/plain")),
      AgentStream.fromPull { () =>
        val next = remaining
        remaining = None
        Future.successful(next)
      }
    )
  }

  private val handler: HttpHandler.Handler = request =>
    request.path match {
      case "/scala/echo" =>
        Future.successful(
          HttpResponse(
            UShort(200),
            List(
              HttpHeader.ascii("x-method", request.method),
              HttpHeader.ascii("x-path", request.path),
              HttpHeader.ascii("x-query", request.query.getOrElse("<absent>")),
              HttpHeader.ascii("set-cookie", "first=one"),
              HttpHeader("set-cookie", Array[Byte](115, 61, 128.toByte))
            ),
            request.body
          )
        )
      case "/scala/dependency" =>
        HttpDocumentsClient.get("from-router").update(config.value.greeting).map(_ => text("updated"))
      case "/scala/fail-before" => Future.failed(new IllegalStateException("handler failure"))
      case "/scala/fail-after"  =>
        var first = true
        Future.successful(
          HttpResponse(
            UShort(200),
            Nil,
            request.body.map { bytes =>
              if (first) { first = false; bytes }
              else throw new IllegalStateException("producer failure")
            }
          )
        )
      case "/scala/head" | "/scala/204" | "/scala/205" | "/scala/304" =>
        val status = request.path.stripPrefix("/scala/") match {
          case "head" => 200
          case value  => value.toInt
        }
        Future.successful(
          HttpResponse(
            UShort(status),
            List(HttpHeader.ascii("content-length", "17")),
            AgentStream.fromPull(() => Future.failed(new IllegalStateException("body must not be polled")))
          )
        )
      case "/scala/invalid-head" => Future.successful(text("invalid").copy(status = UShort(99)))
      case "/scala/early"        => request.body.close().map(_ => text("early"))
      case _                     => request.body.close().map(_ => text(config.value.greeting))
    }

  def serve(request: HttpRequest, principal: golem.Principal): Future[HttpResponse] =
    HttpHandler.ensuring(handler)(() => Future.successful(())).apply(request)

  def describe(): Future[String] = Future.successful(
    """{"openapi":"3.1.0","info":{"title":"Scala website","version":"1"},"paths":{"/echo":{"post":{"operationId":"scalaEcho","responses":{"200":{"description":"Streamed echo"}}}}}}"""
  )
}

@agentDefinition(
  mount = "/scala-documents/{owner}",
  exposeFiles = Array(("/latest", "/public/latest.txt"), ("/*", "/public/$1"))
)
trait HttpDocuments extends BaseAgent {
  class Id(val owner: String)
  def update(text: String): Future[Unit]
}

@agentImplementation()
final class HttpDocumentsImpl(owner: String) extends HttpDocuments {
  HttpExampleFs.mkdirSync("/public", js.Dynamic.literal(recursive = true))
  HttpExampleFs.writeFileSync("/public/latest.txt", s"created:$owner")

  def update(text: String): Future[Unit] = {
    HttpExampleFs.writeFileSync("/public/latest.txt", text)
    Future.successful(())
  }
}

@js.native
@JSImport("node:fs", JSImport.Namespace)
private object HttpExampleFs extends js.Object {
  def mkdirSync(path: String, options: js.Object): Unit = js.native
  def writeFileSync(path: String, text: String): Unit   = js.native
}
