// Copyright 2024-2026 Golem Cloud
// Licensed under the Golem Source License v1.1 (the "License");
// you may not use this file except in compliance with the License.
package golem.config

import golem.reflection.SchemaRef
import golem.schema.*
import zio.blocks.schema.json.Json
import zio.test.*
import scala.collection.immutable.ListMap

object StaticConfigCorpusSpec extends ZIOSpecDefault {
  private val corpus = {
    val input = getClass.getResourceAsStream("/v1.json")
    require(input != null, "config-conformance/v1.json resource is missing")
    try ujson.read(input)
    finally input.close()
  }

  // Decode the independent corpus notation, not SDK-serialized metadata.
  private def shape(v: ujson.Value): SchemaType = {
    val b = v("value")
    v("kind").str match {
      case "string" => t.string
      case "bool"   => t.bool
      case "s32"    => t.s32
      case "u32"    => t.u32
      case "ref"    => t.ref(b("id").str)
      case "option" => t.option(shape(b("inner")))
      case "list"   => t.list(shape(b("element")))
      case "tuple"  => t.tuple(b("elements").arr.toList.map(shape))
      case "record" => t.record(b("fields").arr.toList.map(f => NamedFieldType(f("name").str, shape(f("body")))))
      case "union"  =>
        SchemaType(SchemaTypeBody.UnionType(b("spec")("branches").arr.toList.map { branch =>
          val d    = branch("discriminator")
          val rule = d("rule").str match {
            case "prefix"       => DiscriminatorRule.Prefix(d("value")("prefix").str)
            case "field-equals" =>
              DiscriminatorRule.FieldEquals(
                FieldDiscriminator(d("value")("fieldName").str, Some(d("value")("literal").str))
              )
          }
          UnionBranch(branch("tag").str, shape(branch("body")), rule)
        }))
      case other => throw new IllegalArgumentException(s"Unsupported fixture kind $other")
    }
  }

  private def graph(schema: ujson.Value, declaration: ujson.Value): SchemaGraph = {
    val defs = schema("graph").obj.get("defs").toList.flatMap(_.arr.toList).map { d =>
      d("id").str -> SchemaTypeDef(shape(d("body")), d.obj.get("name").map(_.str))
    }
    SchemaGraph(ListMap(defs*), shape(declaration("value_type")))
  }

  override def spec = suite("StaticConfigCorpusSpec")(
    test("Scala-produced record and tuple defaults match the independent reference corpus") {
      val produced  = ConfigSchema[StaticConfigSpec.Values].describe(Nil)
      val reference = corpus("schemas")("reference")("declarations").arr.toList
      assertTrue(List("settings", "slots", "omitted").forall { name =>
        val expected = reference.find(_("path").arr.toList.map(_.str) == List(name)).get
        val actual   = produced.find(_.path == List(name)).get
        actual.defaultValue.map(ujson.read(_)) == expected.obj.get("default_value").map(v => ujson.read(v.str))
      })
    },
    test("canonical defaults retain reference graphs, union discriminators and recursive shapes") {
      val names = List("reference", "recordUnion", "recursive", "optionalDefault", "emptyValues", "canonicalName")
      assertTrue(names.forall { name =>
        val schema = corpus("schemas")(name)
        schema("declarations").arr.forall { declaration =>
          declaration.obj.get("default_value").forall { default =>
            val ref = SchemaRef(graph(schema, declaration))
            ref
              .validateJson(Json.parseUnsafe(default.str))
              .flatMap(v => ref.unpackJson(v).left.map(List(_)))
              .exists(out => ujson.read(out.print) == ujson.read(default.str))
          }
        }
      })
    }
  )
}
