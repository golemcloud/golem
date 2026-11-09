package golem.bridge.runtime

import golem.bridge.runtime.json.Json
import java.io.File
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

class StreamValueTest {
    @Test
    fun a_provisional_stream_reference_round_trips() {
        val value = SchemaValue.StreamRef("00112233-4455-4677-8899-aabbccddeeff", null)
        val json = """{"kind":"stream","value":{"provisionalRef":"00112233-4455-4677-8899-aabbccddeeff"}}"""
        assertEquals(json, SchemaValueJson.toJson(value).render())
        assertEquals(value, SchemaValueJson.fromJson(Json.parse(json)))
    }

    @Test
    fun a_stable_stream_token_round_trips() {
        val value = SchemaValue.StreamRef(null, "opaque-stream-token")
        val json = """{"kind":"stream","value":{"streamToken":"opaque-stream-token"}}"""
        assertEquals(json, SchemaValueJson.toJson(value).render())
        assertEquals(value, SchemaValueJson.fromJson(Json.parse(json)))
    }

    @Test
    fun a_stream_reference_must_carry_exactly_one_reference() {
        assertFailsWith<BridgeException> { SchemaValueJson.toJson(SchemaValue.StreamRef(null, null)) }
        assertFailsWith<BridgeException> { SchemaValueJson.toJson(SchemaValue.StreamRef("a", "b")) }
        assertFailsWith<BridgeException> { SchemaValueJson.fromJson(Json.parse("""{"kind":"stream","value":{}}""")) }
        assertFailsWith<BridgeException> {
            SchemaValueJson.fromJson(Json.parse("""{"kind":"stream","value":{"provisionalRef":"a","streamToken":"b"}}"""))
        }
    }

    @Test
    fun every_schema_value_vector_round_trips_to_its_canonical_bytes() {
        val fixture = Json.parse(File("schema-values.json").readText())
        assertEquals("1", Json.asNumberLiteral(Json.requireField(fixture, "version")))
        val vectors = Json.asArray(Json.requireField(fixture, "vectors"))
        assertTrue(vectors.isNotEmpty())
        for (vector in vectors) {
            val name = Json.asString(Json.requireField(vector, "name"))
            val canonical = Json.asString(Json.requireField(vector, "canonical"))
            assertEquals(canonical, SchemaValueJson.fromJson(Json.parse(canonical)).let(SchemaValueJson::render), name)
        }
    }
}
