// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
package golem.runtime.http

import golem.runtime._
import golem.schema._
import zio.test._
import scala.collection.immutable.ListMap

object HttpCorpusSpec extends ZIOSpecDefault {
  private val cases = {
    val input = getClass.getResourceAsStream("/corpus.json")
    require(input != null, "shared HTTP corpus resource is missing")
    try ujson.read(input)("cases").arr.toList
    finally input.close()
  }

  private def array(value: ujson.Value, key: String): List[ujson.Value] =
    value.obj.get(key).toList.flatMap(_.arr.toList)

  // Fixture notation describes semantic types, independently of the SDK codecs.
  private def schema(name: String, input: ujson.Value): SchemaGraph = {
    def record(fields: (String, SchemaType)*): SchemaType = t.record(fields.toList.map { case (n, s) =>
      NamedFieldType(n, s)
    })
    def shape(name: String): SchemaType = name match {
      case "string"         => t.string
      case "list<u8>"       => t.list(t.u8)
      case "stream<string>" => SchemaType(SchemaTypeBody.StreamType(Some(t.string)))
      case "HttpRequest"    =>
        record(
          "method"    -> t.string,
          "scheme"    -> t.string,
          "authority" -> t.string,
          "path"      -> t.string,
          "query"     -> t.option(t.string),
          "headers"   -> headers,
          "body"      -> input.obj
            .get("schema_overrides")
            .flatMap(_.obj.get("HttpRequest"))
            .flatMap(_.obj.get("body"))
            .map(v => shape(v.str))
            .getOrElse(body)
        )
      case "HttpResponse" => record("status" -> t.u16, "headers" -> headers, "body" -> body)
      case other          => throw new IllegalArgumentException(s"unsupported corpus type: $other")
    }
    def headers: SchemaType = t.list(record("name" -> t.string, "value" -> t.list(t.u8)))
    def body: SchemaType    = SchemaType(SchemaTypeBody.StreamType(Some(t.list(t.u8))))
    input.obj.get("schema_aliases").flatMap(_.obj.get(name)) match {
      case Some(target) => SchemaGraph(ListMap(name -> SchemaTypeDef(shape(target.str))), t.ref(name))
      case None         => SchemaBuilder.graphOf(_ => shape(name))
    }
  }

  private def metadata(input: ujson.Value): AgentMetadata = {
    def parameters(values: List[ujson.Value]): InputMetadata = InputMetadata(
      values.map(p => ParameterMetadata(p(0).str, FieldSource.UserSupplied, schema(p(1).str, input)))
    )
    def mappings(key: String): List[FileMapping] =
      FileMappingParser.compile(array(input, key).map(p => (p(0).str, p(1).str))).toOption.get
    val methods = array(input, "methods").map { m =>
      MethodMetadata(
        m("name").str,
        None,
        None,
        None,
        parameters(array(m, "input")),
        OutputMetadata.Single(schema(m("output").str, input)),
        httpEndpoints = array(m, "bindings").map { binding =>
          HttpEndpointDetails(
            if (binding(0).str == "Any") HttpMethod.Any else HttpMethod.fromString(binding(0).str).toOption.get,
            HttpRouteParser.parsePath(binding(1).str).toOption.get,
            Nil,
            Nil,
            m.obj.get("endpoint_auth").map(_ => true),
            None
          )
        }
      )
    }
    AgentMetadata(
      "corpus-agent",
      if (input("kind").str == "http-router") AgentTypeKind.HttpRouter else AgentTypeKind.Regular,
      None,
      Some(input("mode").str),
      methods,
      ConstructorMetadata(None, "corpus", None, parameters(array(input, "constructor"))),
      httpMount = array(input, "mounts").headOption.map(m =>
        HttpMountDetails(
          HttpRouteParser.parsePath(m.str).toOption.get,
          false,
          input.obj.get("phantom").exists(_.bool),
          Nil,
          Nil,
          mappings("static_bindings"),
          mappings("filesystem_bindings"),
          input.obj.get("provider").map(_.str)
        )
      ),
      snapshotting =
        if (input.obj.get("snapshot").exists(_.bool)) Snapshotting.Enabled(SnapshottingConfig.Default)
        else Snapshotting.Disabled
    )
  }

  def spec = suite("HTTP shared corpus")(
    suite("metadata")(cases.filter(_("suite").str == "metadata").map { c =>
      test(c("id").str) {
        val result   = HttpAgentValidation.validate(metadata(c("input")))
        val expected = c("expect").obj.get("error").map(v => Left(v.str)).getOrElse(Right(()))
        assertTrue(result == expected)
      }
    }),
    suite("mapping")(cases.filter(_("suite").str == "mapping").map { c =>
      test(c("id").str) {
        val result   = FileMappingParser.compile(c("input")("mappings").arr.toList.map(p => (p(0).str, p(1).str)))
        val observed = result.map(mappings =>
          ujson.Arr.from(mappings.map {
            case FileMapping.Exact(public, file) =>
              ujson.Obj("Exact" -> ujson.Obj("public_path" -> ujson.Arr.from(public), "file_path" -> file))
            case FileMapping.Subtree(public, root) =>
              ujson.Obj("Subtree" -> ujson.Obj("public_prefix" -> ujson.Arr.from(public), "filesystem_root" -> root))
          })
        )
        val expected = c("expect").obj.get("error").map(v => Left(v.str)).getOrElse(Right(c("expect")("compiled")))
        assertTrue(observed == expected)
      }
    })
  )
}
