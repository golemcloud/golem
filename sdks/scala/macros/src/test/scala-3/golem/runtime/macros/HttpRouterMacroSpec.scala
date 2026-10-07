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
    cors = Array("https://example.com")
  )
  trait Website {
    @httpHandler def arbitraryName(request: HttpRequest, principal: golem.Principal): Future[HttpResponse]
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
    exposeFiles = Array(("/latest", "/public/latest.txt"), ("/*", "/public/$1"))
  )
  trait Documents {
    class Id(val owner: String)
    @endpoint("GET", "/latest") def latest(): String
  }

  @agentDefinition(mount = "/c/{customer}/{instance}", phantomIdPath = "instance")
  trait SelectedPath {
    class Id(val customer: String)
    @endpoint("GET", "/read") def read(): String
  }
  @agentDefinition(mount = "/c/{customer}/{instance}", phantomIdPath = "instance", phantomIdOptional = true)
  trait OptionalPath {
    class Id(val customer: String)
    @endpoint("GET", "/read") def read(): String
  }
  @agentDefinition(mount = "/c/{customer}", phantomIdQuery = "instance")
  trait SelectedQuery {
    class Id(val customer: String)
    @endpoint("GET", "/read") def read(): String
  }
  @agentDefinition(mount = "/c/{customer}", phantomIdQuery = "instance", phantomIdOptional = true)
  trait OptionalQuery {
    class Id(val customer: String)
    @endpoint("GET", "/read") def read(): String
  }

  @agentDefinition(mount = "/c/{customer}", phantomIdQuery = " instance ")
  trait PaddedQuery {
    class Id(val customer: String)
    @endpoint("GET", "/read") def read(): String
  }
  @agentDefinition(mount = "/c/{customer}", phantomIdQuery = " ", phantomAgent = true)
  trait WhitespaceQuery {
    class Id(val customer: String)
    @endpoint("GET", "/read") def read(): String
  }

  def spec = suite("HttpRouterMacroSpec")(
    test("query selector names are preserved exactly, including whitespace-only names") {
      val padded     = AgentDefinitionMacro.generateWire[PaddedQuery]
      val whitespace = AgentDefinitionMacro.generateWire[WhitespaceQuery]
      assertTrue(
        padded.httpMount.get.phantomIdBinding.contains(PhantomIdBinding.Query(" instance ")),
        whitespace.httpMount.get.phantomIdBinding.contains(PhantomIdBinding.Query(" ")),
        whitespace.httpMount.get.phantomAgent
      )
    },
    test("selector annotations survive compiled wire metadata without constructor arguments") {
      val path          = AgentDefinitionMacro.generateWire[SelectedPath]
      val optionalPath  = AgentDefinitionMacro.generateWire[OptionalPath]
      val query         = AgentDefinitionMacro.generateWire[SelectedQuery]
      val optionalQuery = AgentDefinitionMacro.generateWire[OptionalQuery]
      assertTrue(
        path.httpMount.get.phantomIdBinding.contains(PhantomIdBinding.Path("instance")),
        optionalPath.httpMount.get.phantomIdBinding.contains(PhantomIdBinding.Path("instance", true)),
        query.httpMount.get.phantomIdBinding.contains(PhantomIdBinding.Query("instance")),
        optionalQuery.httpMount.get.phantomIdBinding.contains(PhantomIdBinding.Query("instance", true))
      )
    },
    test("phantom selectors reject ambiguous ownership and restricted modes at compile time") {
      val extra: List[scala.compiletime.testing.Error] = scala.compiletime.testing.typeCheckErrors("""
        import golem.runtime.annotations.*
        @agentDefinition(mount="/c/{instance}/{other}", phantomIdPath="instance")
        trait A { class Id(); @endpoint("GET", "/read") def read(): String }
        golem.runtime.macros.AgentDefinitionMacro.generateWire[A]
      """)
      val conflict: List[scala.compiletime.testing.Error] = scala.compiletime.testing.typeCheckErrors("""
        import golem.runtime.annotations.*
        @agentDefinition(mount="/c/{instance}", phantomIdPath="instance")
        trait A { class Id(val instance: String); @endpoint("GET", "/read") def read(): String }
        golem.runtime.macros.AgentDefinitionMacro.generateWire[A]
      """)
      val ephemeral: List[scala.compiletime.testing.Error] = scala.compiletime.testing.typeCheckErrors("""
        import golem.runtime.annotations.*
        @agentDefinition(mount="/c", phantomIdQuery="instance", mode=DurabilityMode.Ephemeral)
        trait A { class Id(); @endpoint("GET", "/read") def read(): String }
        golem.runtime.macros.AgentDefinitionMacro.generateWire[A]
      """)
      assertTrue(
        extra.exists(_.message.contains("other")),
        conflict.exists(_.message.contains("constructor parameter")),
        ephemeral.exists(_.message.contains("durable"))
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
        )
      )
    },
    test("reject caller-dependent live-file identities even when Principal is not mounted") {
      val errors: List[scala.compiletime.testing.Error] = scala.compiletime.testing.typeCheckErrors("""
        import golem.runtime.annotations.*
        import golem.runtime.macros.AgentDefinitionMacro
        @agentDefinition(mount = "/", exposeFiles = Array(("/*", "/public/$1")))
        trait Bad { class Id(val principal: golem.Principal) }
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
