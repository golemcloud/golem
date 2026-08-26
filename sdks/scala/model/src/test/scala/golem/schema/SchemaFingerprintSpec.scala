package golem.schema

import scala.collection.immutable.ListMap
import zio.test._

object SchemaFingerprintSpec extends ZIOSpecDefault {
  private def vector(graph: SchemaGraph, element: Option[SchemaType], length: Int, hex: String) = {
    val bytes = SchemaFingerprintV1.canonicalBytes(graph, element)
    val hash  = SchemaFingerprintV1.compute(graph, element)
    assertTrue(bytes.map(_.length) == Right(length), hash.map(_.toHex) == Right(hex))
  }

  def spec = suite("SchemaFingerprintV1")(
    test("matches all five platform golden vectors") {
      val empty  = SchemaGraph(ListMap.empty, t.string)
      val nodeId = "example.node"
      val node   = SchemaTypeDef(
        t.record(List(t.field("value", t.string), t.field("next", t.option(t.ref(nodeId))))),
        Some("Node")
      )
      val recursive   = SchemaGraph(ListMap(nodeId -> node), t.ref(nodeId))
      val constrained = SchemaType(
        SchemaTypeBody.TextType(TextRestrictions(Some(List("fr", "en")), Some(1), Some(64), Some("^[a-z]+$"))),
        MetadataEnvelope(Some("text"), List("z", "a"), List("\"hello\""), Some("use-v2"), Some(Role.Other("prompt")))
      )
      vector(empty, None, 37, "b50494cf0f33961c703d5f6e6af3d3159e528c4d09c1d801172cdf8f022dcafa") &&
      vector(empty, Some(t.string), 37, "61c50c0a3c6ffd63529621ada78afc0d4d8e5fe691f8b0993035f847c660a307") &&
      vector(empty, Some(t.list(t.string)), 45, "4939707f8ef97e9d4b31b568332eaf5a3011f2be7c358f7546966fadfb9416d4") &&
      vector(
        recursive,
        Some(recursive.root),
        140,
        "3931585d2d02a2b7d5c99e3da1082ac8fe904c535e2700bd45e29a95ff2399fa"
      ) &&
      vector(empty, Some(constrained), 87, "b985cdb5445862be90e8dca06bbfa9c46b50cf40edc84ed34205bb3a214c5bb0")
    },
    test("rejects dangling refs and duplicate set values") {
      val dangling  = SchemaGraph(ListMap.empty, t.ref("missing"))
      val duplicate = SchemaType(SchemaTypeBody.TextType(TextRestrictions(languages = Some(List("en", "en")))))
      assertTrue(SchemaFingerprintV1.compute(dangling, Some(dangling.root)).isLeft) &&
      assertTrue(SchemaFingerprintV1.compute(SchemaGraph(ListMap.empty, duplicate), Some(duplicate)).isLeft)
    },
    test("excludes unreachable definitions") {
      val extra = SchemaGraph(ListMap("unused" -> SchemaTypeDef(t.bool)), t.string)
      val empty = SchemaGraph(ListMap.empty, t.string)
      assertTrue(
        SchemaFingerprintV1.compute(extra, Some(t.string)) == SchemaFingerprintV1.compute(empty, Some(t.string))
      )
    },
    test("reports invalid UTF-8 in reachable definition IDs through the declared error channel") {
      val invalidId = new String(Array(0xd800.toChar))
      val validId   = "valid"
      val graph     = SchemaGraph(
        ListMap(invalidId -> SchemaTypeDef(t.ref(validId)), validId -> SchemaTypeDef(t.string)),
        t.ref(invalidId)
      )
      assertTrue(SchemaFingerprintV1.compute(graph, Some(graph.root)).isLeft)
    }
  )
}
