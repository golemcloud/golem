// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
package golem.runtime.http

import golem.runtime._
import golem.schema._
import zio.test._

object HttpAgentIdMountSpec extends ZIOSpecDefault {
  private val field = ParameterMetadata(
    "customer",
    FieldSource.UserSupplied,
    SchemaGraph(scala.collection.immutable.ListMap.empty, t.string)
  )
  private val path = HttpMountDetails(
    List(PathSegment.Literal("agents"), PathSegment.PathVariable("id")),
    false,
    false,
    Nil,
    Nil,
    Nil,
    List(FileMapping.Subtree(Nil, "/public")),
    None,
    agentIdBinding = Some(AgentIdBinding.Path("id")),
    createRoute = Some(List(PathSegment.SystemVariable("agent-type"), PathSegment.Literal("create")))
  )
  private val query =
    path.copy(pathPrefix = List(PathSegment.Literal("agents")), agentIdBinding = Some(AgentIdBinding.Query("id")))
  private def agent(mount: HttpMountDetails) = AgentMetadata(
    "Selected",
    AgentTypeKind.Regular,
    None,
    Some("durable"),
    Nil,
    ConstructorMetadata(None, "", None, InputMetadata(List(field))),
    Some(mount)
  )
  private def valid(mount: HttpMountDetails) = HttpAgentValidation.validate(agent(mount)).isRight

  def spec = suite("HttpAgentIdMountSpec")(
    test("reserve controls for inferred input and output streams, respecting explicit transport overrides") {
      val stream = SchemaType(SchemaTypeBody.StreamType(Some(t.string)))
      val graph  = SchemaGraph(scala.collection.immutable.ListMap("events" -> SchemaTypeDef(stream)), t.ref("events"))
      val checks = for {
        name  <- List("offset", "cursor", "live", "instance")
        shape <- List("input", "output", "recordOutput", "plain")
        mode  <- List(
                  None,
                  Some(HttpRouteMode.DurableStreams),
                  Some(HttpRouteMode.Plain),
                  Some(HttpRouteMode.Sse),
                  Some(HttpRouteMode.WebSocket)
                )
        options <- List(None, Some(DurableStreamRouteOptions(Nil, None)))
      } yield {
        val endpoint =
          HttpEndpointDetails(HttpMethod.Post, Nil, Nil, Nil, None, None, durableStreams = options, mode = mode)
        val output = shape match {
          case "output"       => OutputMetadata.Single(graph)
          case "recordOutput" =>
            OutputMetadata.Single(
              graph.copy(root = SchemaType(SchemaTypeBody.RecordType(List(NamedFieldType("events", t.ref("events"))))))
            )
          case _ => OutputMetadata.Unit
        }
        val method = MethodMetadata(
          "events",
          None,
          None,
          None,
          InputMetadata(if (shape == "input") List(field.copy(graph = graph)) else Nil),
          output,
          List(endpoint)
        )
        val reserved =
          name != "instance" && (mode.contains(HttpRouteMode.DurableStreams) || (mode.isEmpty && shape != "plain"))
        HttpAgentValidation
          .validate(agent(query.copy(agentIdBinding = Some(AgentIdBinding.Query(name)))).copy(methods = List(method)))
          .isLeft == reserved
      }
      assertTrue(checks.forall(identity))
    },
    test("path and query ID mounts allow files and a nonempty constructor without constructor URL bindings") {
      assertTrue(valid(path), valid(query), valid(path.copy(createRoute = Some(Nil))))
    },
    test("reject mixed constructor captures, duplicate or missing ID captures, and phantomAgent") {
      assertTrue(
        !valid(path.copy(pathPrefix = path.pathPrefix :+ PathSegment.PathVariable("customer"))),
        !valid(query.copy(pathPrefix = path.pathPrefix)),
        !valid(path.copy(pathPrefix = path.pathPrefix :+ PathSegment.PathVariable("id"))),
        !valid(path.copy(pathPrefix = Nil)),
        !valid(query.copy(phantomAgent = true)),
        !valid(query.copy(agentIdBinding = Some(AgentIdBinding.Query(""))))
      )
    },
    test("create requires an ID mount and an exact literal/system path") {
      assertTrue(
        !valid(path.copy(agentIdBinding = None)),
        !valid(query.copy(createRoute = Some(List(PathSegment.PathVariable("id"))))),
        !valid(query.copy(createRoute = Some(List(PathSegment.RemainingPathVariable("tail")))))
      )
    },
    test("ID mounts require a regular durable owner") {
      assertTrue(
        HttpAgentValidation.validate(agent(query).copy(mode = Some("ephemeral"))).isLeft,
        HttpAgentValidation.validate(agent(query).copy(kind = AgentTypeKind.HttpRouter)).isLeft
      )
    },
    test("reserve query ID against explicit and implicit method query inputs, but not header inputs") {
      val endpoint = HttpEndpointDetails(HttpMethod.Get, Nil, Nil, Nil, None, None)
      val method   = MethodMetadata(
        "read",
        None,
        None,
        None,
        InputMetadata(List(field.copy(name = "id"))),
        OutputMetadata.Unit,
        List(endpoint)
      )
      def check(e: HttpEndpointDetails) =
        HttpAgentValidation.validate(agent(query).copy(methods = List(method.copy(httpEndpoints = List(e)))))
      assertTrue(
        check(endpoint).isLeft,
        check(endpoint.copy(queryVars = List(QueryVariable("id", "other")))).isLeft,
        check(endpoint.copy(headerVars = List(HeaderVariable("x-id", "id")))).isRight,
        check(endpoint.copy(httpMethod = HttpMethod.Post)).isRight,
        HttpAgentValidation
          .validate(agent(query).copy(methods = List(method.copy(input = InputMetadata(List(field))))))
          .isRight
      )
    },
    test("query ID excludes native input streams but still reserves explicit query wire names") {
      val stream = SchemaType(SchemaTypeBody.StreamType(Some(t.string)))
      val graphs = List(
        SchemaGraph(scala.collection.immutable.ListMap.empty, stream),
        SchemaGraph(scala.collection.immutable.ListMap("events" -> SchemaTypeDef(stream)), t.ref("events"))
      )
      val checks = for {
        graph <- graphs
        verb  <- List(HttpMethod.Get, HttpMethod.Head, HttpMethod.Delete)
      } yield {
        val endpoint = HttpEndpointDetails(verb, Nil, Nil, Nil, None, None)
        val method   = MethodMetadata(
          "events",
          None,
          None,
          None,
          InputMetadata(List(field.copy(name = "id", graph = graph))),
          OutputMetadata.Unit,
          List(endpoint)
        )
        def check(e: HttpEndpointDetails) =
          HttpAgentValidation.validate(agent(query).copy(methods = List(method.copy(httpEndpoints = List(e)))))
        check(endpoint).isRight &&
        check(endpoint.copy(queryVars = List(QueryVariable("id", "id")))).isLeft
      }
      assertTrue(checks.forall(identity))
    },
    test("key mounts and per-request phantomAgent lifecycle retain constructor capture semantics") {
      val key =
        path.copy(pathPrefix = List(PathSegment.PathVariable("customer")), agentIdBinding = None, createRoute = None)
      assertTrue(
        valid(key),
        !valid(key.copy(pathPrefix = Nil)),
        valid(key.copy(phantomAgent = true, filesystemBindings = Nil))
      )
    }
  )
}
