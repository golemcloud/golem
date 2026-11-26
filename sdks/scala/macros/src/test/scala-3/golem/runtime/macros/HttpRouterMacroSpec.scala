// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
package golem.runtime.macros

import golem.runtime.annotations.*
import golem.runtime.http.*
import golem.runtime.{AgentTypeKind, Snapshotting}
import scala.concurrent.Future
import zio.test.*

object HttpRouterMacroSpec extends ZIOSpecDefault {
  @httpRouter(
    "website",
    "/web",
    staticBindings = Array(("/assets/*", "/one/$1"), ("/assets/logo", "/logo"), ("/assets/*", "/two/$1")),
    auth = true,
    cors = Array("https://example.com"),
    fileResponseHeaders = Array(("Content-Security-Policy", "default-src 'self'"), ("Referrer-Policy", "same-origin"))
  )
  trait Website {
    @httpHandler def arbitraryName(request: HttpRequest, context: golem.InvocationContext): Future[HttpResponse]
    @openApiProvider def description(): Future[String]
  }
  @httpRouter("files", "/", staticBindings = Array(("/", "/index.html")))
  trait Files
  @httpRouter("provider", "/spec")
  trait Provider { @openApiProvider def customName(): String }
  @httpRouter("handler", "/api")
  trait Handler { @httpHandler def serve(request: HttpRequest): HttpResponse }
  @httpRouter("empty", "/empty")
  trait Empty

  @agentDefinition(
    mount = "/doc%75ments/{owner}",
    exposeFiles = Array(("/latest", "/public/latest.txt"), ("/*", "/public/$1")),
    fileResponseHeaders = Array(("Content-Security-Policy", "default-src 'none'"), ("Referrer-Policy", "no-referrer"))
  )
  trait Documents {
    class Id(val owner: String)
    @endpoint("GET", "/latest") def latest(): String
  }

  @agentDefinition(
    mount = "/c/{instance}",
    agentIdPath = "instance",
    createRoute = "/{agent-type}/create",
    exposeFiles = Array(("/*", "/public/$1"))
  )
  trait SelectedPath {
    class Id(val customer: String)
    @endpoint("GET", "/read") def read(): String
  }
  @agentDefinition(mount = "/c", agentIdQuery = "instance", createRoute = "/c/create")
  trait SelectedQuery {
    class Id(val customer: String)
    @endpoint("GET", "/read") def read(): String
  }
  @agentDefinition(mount = "/c", agentIdQuery = " instance ")
  trait PaddedQuery {
    class Id(val customer: String)
    @endpoint("GET", "/read") def read(): String
  }
  @agentDefinition(mount = "/c", agentIdQuery = " ")
  trait WhitespaceQuery {
    class Id(val customer: String)
    @endpoint("GET", "/read") def read(): String
  }

  def spec = suite("HttpRouterMacroSpec")(
    test("agent endpoints reject tool-only declaration options") {
      val prefix: List[scala.compiletime.testing.Error] = scala.compiletime.testing.typeCheckErrors("""
        import golem.runtime.annotations.*
        @agentDefinition(mount="/a") trait A {
          class Id()
          @endpoint("POST", "/clean")
          @endpoint("POST", "/run", prefix="/ignored") def run(): Unit
        }
        golem.runtime.macros.AgentDefinitionMacro.generateWire[A]
      """)
      val id: List[scala.compiletime.testing.Error] = scala.compiletime.testing.typeCheckErrors("""
        import golem.runtime.annotations.*
        @agentDefinition(mount="/a") trait A {
          class Id()
          @endpoint("POST", "/clean")
          @endpoint("POST", "/run", routeId="ignored") def run(): Unit
        }
        golem.runtime.macros.AgentDefinitionMacro.generate[A]
      """)
      val headers: List[scala.compiletime.testing.Error] = scala.compiletime.testing.typeCheckErrors("""
        import golem.runtime.annotations.*
        @agentDefinition(mount="/a") trait A {
          class Id()
          @endpoint("POST", "/clean")
          @endpoint("POST", "/run", responseHeaders=Array(("X-Result", "ignored"))) def run(): Unit
        }
        golem.runtime.macros.AgentDefinitionMacro.generateWire[A]
      """)
      assertTrue(
        prefix.exists(_.message.contains("tool-only endpoint option")),
        id.exists(_.message.contains("tool-only endpoint option")),
        headers.exists(_.message.contains("tool-only endpoint option"))
      )
    },
    test("agent checks every endpoint in either annotation order, including explicit empty options") {
      val prefix: List[scala.compiletime.testing.Error] = scala.compiletime.testing.typeCheckErrors("""
        import golem.runtime.annotations.*
        @agentDefinition(mount="/a") trait A {
          class Id()
          @endpoint("POST", "/run", prefix="")
          @endpoint("POST", "/clean") def run(): Unit
        }
        golem.runtime.macros.AgentDefinitionMacro.generateWire[A]
      """)
      val id: List[scala.compiletime.testing.Error] = scala.compiletime.testing.typeCheckErrors("""
        import golem.runtime.annotations.*
        @agentDefinition(mount="/a") trait A {
          class Id()
          @endpoint("POST", "/run", routeId="")
          @endpoint("POST", "/clean") def run(): Unit
        }
        golem.runtime.macros.AgentDefinitionMacro.generate[A]
      """)
      val headers: List[scala.compiletime.testing.Error] = scala.compiletime.testing.typeCheckErrors("""
        import golem.runtime.annotations.*
        @agentDefinition(mount="/a") trait A {
          class Id()
          @endpoint("POST", "/run", responseHeaders=Array.empty)
          @endpoint("POST", "/clean") def run(): Unit
        }
        golem.runtime.macros.AgentDefinitionMacro.generateWire[A]
      """)
      assertTrue(
        prefix.exists(_.message.contains("tool-only endpoint option")),
        id.exists(_.message.contains("tool-only endpoint option")),
        headers.exists(_.message.contains("tool-only endpoint option"))
      )
    },
    test("query selector names are preserved exactly, including whitespace-only names") {
      val padded     = AgentDefinitionMacro.generateWire[PaddedQuery]
      val whitespace = AgentDefinitionMacro.generateWire[WhitespaceQuery]
      assertTrue(
        padded.httpMount.get.agentIdBinding.contains(AgentIdBinding.Query(" instance ")),
        whitespace.httpMount.get.agentIdBinding.contains(AgentIdBinding.Query(" "))
      )
    },
    test("selector annotations survive compiled wire metadata without constructor arguments") {
      val path  = AgentDefinitionMacro.generateWire[SelectedPath]
      val query = AgentDefinitionMacro.generateWire[SelectedQuery]
      assertTrue(
        path.httpMount.get.agentIdBinding.contains(AgentIdBinding.Path("instance")),
        path.httpMount.get.createRoute
          .contains(List(PathSegment.SystemVariable("agent-type"), PathSegment.Literal("create"))),
        path.httpMount.get.filesystemBindings.nonEmpty,
        query.httpMount.get.agentIdBinding.contains(AgentIdBinding.Query("instance")),
        query.httpMount.get.createRoute.contains(List(PathSegment.Literal("c"), PathSegment.Literal("create")))
      )
    },
    test("ID bindings reject ambiguous ownership and restricted modes at compile time") {
      val extra: List[scala.compiletime.testing.Error] = scala.compiletime.testing.typeCheckErrors("""
        import golem.runtime.annotations.*
        @agentDefinition(mount="/c/{instance}/{other}", agentIdPath="instance")
        trait A { class Id(); @endpoint("GET", "/read") def read(): String }
        golem.runtime.macros.AgentDefinitionMacro.generateWire[A]
      """)
      val conflict: List[scala.compiletime.testing.Error] = scala.compiletime.testing.typeCheckErrors("""
        import golem.runtime.annotations.*
        @agentDefinition(mount="/c/{instance}", agentIdPath="instance", phantomAgent=true)
        trait A { class Id(val instance: String); @endpoint("GET", "/read") def read(): String }
        golem.runtime.macros.AgentDefinitionMacro.generateWire[A]
      """)
      val ephemeral: List[scala.compiletime.testing.Error] = scala.compiletime.testing.typeCheckErrors("""
        import golem.runtime.annotations.*
        @agentDefinition(mount="/c", agentIdQuery="instance", mode=DurabilityMode.Ephemeral)
        trait A { class Id(); @endpoint("GET", "/read") def read(): String }
        golem.runtime.macros.AgentDefinitionMacro.generateWire[A]
      """)
      assertTrue(
        extra.exists(_.message.contains("constructor bindings")),
        conflict.exists(_.message.contains("phantomAgent")),
        ephemeral.exists(_.message.contains("durable"))
      )
    },
    test("create declarations and implicit query collisions fail during wire generation") {
      val keyed: List[scala.compiletime.testing.Error] = scala.compiletime.testing.typeCheckErrors("""
        import golem.runtime.annotations.*
        @agentDefinition(mount="/c/{customer}", createRoute="/c/create")
        trait A { class Id(val customer: String) }
        golem.runtime.macros.AgentDefinitionMacro.generateWire[A]
      """)
      val capture: List[scala.compiletime.testing.Error] = scala.compiletime.testing.typeCheckErrors("""
        import golem.runtime.annotations.*
        @agentDefinition(mount="/c", agentIdQuery="id", createRoute="/c/{id}")
        trait A { class Id(val customer: String) }
        golem.runtime.macros.AgentDefinitionMacro.generateWire[A]
      """)
      val implicitQuery: List[scala.compiletime.testing.Error] = scala.compiletime.testing.typeCheckErrors("""
        import golem.runtime.annotations.*
        @agentDefinition(mount="/c", agentIdQuery="id")
        trait A { class Id(val customer: String); @endpoint("GET", "/read") def read(id: String): String }
        golem.runtime.macros.AgentDefinitionMacro.generateWire[A]
      """)
      assertTrue(
        keyed.exists(_.message.contains("requires an agent ID binding")),
        capture.exists(_.message.contains("literal and system")),
        implicitQuery.exists(_.message.contains("conflicts"))
      )
    },
    test("combined router uses normal named methods and ordered mappings") {
      val metadata = AgentDefinitionMacro.generate[Website]
      val wire     = AgentDefinitionMacro.generateWire[Website]
      val mount    = metadata.httpMount.get
      assertTrue(
        wire == golem.runtime.WireAgentMetadata.fromModel(metadata),
        metadata.name == "website",
        metadata.kind == AgentTypeKind.HttpRouter,
        metadata.mode.contains("ephemeral"),
        metadata.constructor.input.parameters.isEmpty,
        metadata.snapshotting == Snapshotting.Disabled,
        mount.authRequired,
        mount.corsAllowedPatterns == List("https://example.com"),
        mount.staticBindings == List(
          FileMapping.Subtree(List("assets"), "/one"),
          FileMapping.Exact(List("assets", "logo"), "/logo"),
          FileMapping.Subtree(List("assets"), "/two")
        ),
        mount.fileResponseHeaders == List(
          FileResponseHeader("Content-Security-Policy", "default-src 'self'"),
          FileResponseHeader("Referrer-Policy", "same-origin")
        ),
        mount.openapiProviderMethod.contains("description"),
        metadata.methods.find(_.name == "arbitraryName").get.httpEndpoints.head.httpMethod == HttpMethod.Any,
        AgentNameMacro.typeName[Website] == "website"
      )
    },
    test("all registration combinations need no Id class or fake method") {
      val files    = AgentDefinitionMacro.generate[Files]
      val provider = AgentDefinitionMacro.generate[Provider]
      val handler  = AgentDefinitionMacro.generate[Handler]
      val empty    = AgentDefinitionMacro.generate[Empty]
      assertTrue(
        files.methods.isEmpty,
        empty.methods.isEmpty,
        provider.methods.map(_.name) == List("customName"),
        handler.methods.map(_.name) == List("serve"),
        files.httpMount.get.pathPrefix.isEmpty
      )
    },
    test("ordinary exposeFiles coexists with typed endpoints") {
      val metadata = AgentDefinitionMacro.generate[Documents]
      assertTrue(
        metadata.kind == AgentTypeKind.Regular,
        metadata.httpMount.get.pathPrefix == List(PathSegment.Literal("documents"), PathSegment.PathVariable("owner")),
        metadata.httpMount.get.filesystemBindings == List(
          FileMapping.Exact(List("latest"), "/public/latest.txt"),
          FileMapping.Subtree(Nil, "/public")
        ),
        metadata.httpMount.get.fileResponseHeaders == List(
          FileResponseHeader("Content-Security-Policy", "default-src 'none'"),
          FileResponseHeader("Referrer-Policy", "no-referrer")
        )
      )
    },
    test("reject caller-dependent live-file identities even when InvocationContext is not mounted") {
      val errors: List[scala.compiletime.testing.Error] = scala.compiletime.testing.typeCheckErrors("""
        import golem.runtime.annotations.*
        import golem.runtime.macros.AgentDefinitionMacro
        @agentDefinition(mount = "/", exposeFiles = Array(("/*", "/public/$1")))
        trait Bad { class Id(val context: golem.InvocationContext) }
        AgentDefinitionMacro.generate[Bad]
      """)
      assertTrue(errors.exists(_.message.contains("filesystem-constructor")))
    },
    test("reject computed mappings rather than silently treating them as empty") {
      val errors: List[scala.compiletime.testing.Error] = scala.compiletime.testing.typeCheckErrors("""
        import golem.runtime.annotations.*
        import golem.runtime.macros.AgentDefinitionMacro
        def mappings(): Array[(String, String)] = Array(("/", "/index.html"))
        @httpRouter("bad", "/", staticBindings = mappings()) trait Bad
        AgentDefinitionMacro.generate[Bad]
      """)
      assertTrue(errors.exists(_.message.contains("literal Array")))
    },
    test("reject parameterized router at compile time") {
      val errors: List[scala.compiletime.testing.Error] = scala.compiletime.testing.typeCheckErrors("""
        import golem.runtime.annotations.*
        import golem.runtime.macros.AgentDefinitionMacro
        @httpRouter("bad", "/") trait Bad { class Id(val name: String) }
        AgentDefinitionMacro.generate[Bad]
      """)
      assertTrue(errors.exists(_.message.contains("router-constructor")))
    },
    test("reject handler policy overrides rather than ignoring them") {
      val errors: List[scala.compiletime.testing.Error] = scala.compiletime.testing.typeCheckErrors("""
        import golem.runtime.annotations.*
        import golem.runtime.http.*
        import golem.runtime.macros.AgentDefinitionMacro
        @httpRouter("bad", "/") trait Bad {
          @httpHandler @endpoint("GET", "/", auth = true)
          def serve(request: HttpRequest): HttpResponse
        }
        AgentDefinitionMacro.generate[Bad]
      """)
      assertTrue(errors.exists(_.message.contains("handler-endpoint-policy")))
    },
    test("reject dynamic mounts and unassigned exported methods") {
      val dynamic: List[scala.compiletime.testing.Error] = scala.compiletime.testing.typeCheckErrors("""
        import golem.runtime.annotations.*
        import golem.runtime.macros.AgentDefinitionMacro
        @httpRouter("bad", "/{name}") trait Bad
        AgentDefinitionMacro.generate[Bad]
      """)
      val extra: List[scala.compiletime.testing.Error] = scala.compiletime.testing.typeCheckErrors("""
        import golem.runtime.annotations.*
        import golem.runtime.macros.AgentDefinitionMacro
        @httpRouter("bad", "/") trait Bad { def extra(): String }
        AgentDefinitionMacro.generate[Bad]
      """)
      assertTrue(
        dynamic.exists(_.message.contains("router-mount")),
        extra.exists(_.message.contains("router-method-role"))
      )
    }
  )
}
