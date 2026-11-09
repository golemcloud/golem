package golem.bridge.runtime

import golem.bridge.runtime.json.Json
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith
import kotlin.test.assertTrue

class JsonTest {
    @Test
    fun numbers_keep_their_exact_literal() {
        val parsed = Json.parse("""[18446744073709551615,-0,1e10,0.1,-12]""")
        assertEquals("""[18446744073709551615,-0,1e10,0.1,-12]""", parsed.render())
    }

    @Test
    fun strings_round_trip_escapes_and_unicode() {
        val text = "quote\" backslash\\ newline\n tab\t control\u0001 snowman\u2603 pair\uD83D\uDE00"
        val rendered = Json.string(text).render()
        assertEquals(text, Json.asString(Json.parse(rendered)))
        assertEquals("\"\\u0001\"", Json.string("\u0001").render())
    }

    @Test
    fun a_unicode_escape_is_decoded() {
        assertEquals("A\u00e9", Json.asString(Json.parse("\"\\u0041\\u00e9\"")))
    }

    @Test
    fun a_signed_unicode_escape_is_rejected() {
        assertFailsWith<BridgeException> { Json.parse("\"\\u+041\"") }
        assertFailsWith<BridgeException> { Json.parse("\"\\u-001\"") }
    }

    @Test
    fun object_members_keep_their_order_and_duplicates() {
        val parsed = Json.parse("""{"b":1,"a":2,"b":3}""")
        assertEquals(listOf("b", "a", "b"), Json.asObject(parsed).map { it.first })
    }

    @Test
    fun an_explicit_null_and_an_absent_member_are_the_same() {
        val parsed = Json.parse("""{"a":null}""")
        assertEquals(null, Json.field(parsed, "a"))
        assertFailsWith<BridgeException> { Json.requireField(parsed, "a") }
    }

    @Test
    fun malformed_input_is_rejected() {
        for (bad in listOf("", "{", "[1,]", "{\"a\"}", "01", "1.", "-", "\"abc", "tru", "{} x", "[1 2]", "\"\\q\"", "\"\u0001\"")) {
            assertFailsWith<BridgeException>("accepted $bad") { Json.parse(bad) }
        }
    }

    @Test
    fun a_lone_surrogate_cannot_be_written() {
        assertFailsWith<BridgeException> { Json.string("a\uD800b").render() }
        assertFailsWith<BridgeException> { Json.string("\uDC00").render() }
        assertEquals("\"\uD83D\uDE00\"", Json.string("\uD83D\uDE00").render())
    }

    @Test
    fun excessive_nesting_is_rejected() {
        assertFailsWith<BridgeException> { Json.parse("[".repeat(10_000) + "]".repeat(10_000)) }
    }
}

class SchemaValueJsonTest {
    private fun sample(): List<SchemaValue> = listOf(
        SchemaValue.Bool(true),
        SchemaValue.S8(-8),
        SchemaValue.S16(-16),
        SchemaValue.S32(-32),
        SchemaValue.S64(Long.MIN_VALUE),
        SchemaValue.U8(255u),
        SchemaValue.U16(65535u),
        SchemaValue.U32(UInt.MAX_VALUE),
        SchemaValue.U64(ULong.MAX_VALUE),
        SchemaValue.F32(1.5f),
        SchemaValue.F64(-0.0),
        SchemaValue.Chr('x'),
        SchemaValue.Str("hi \"there\""),
        SchemaValue.Record(listOf(SchemaValue.S32(1), SchemaValue.Str("a"))),
        SchemaValue.TupleVal(listOf(SchemaValue.Bool(false))),
        SchemaValue.ListVal(listOf(SchemaValue.S32(1), SchemaValue.S32(2))),
        SchemaValue.FixedListVal(listOf(SchemaValue.S8(1))),
        SchemaValue.OptionVal(null),
        SchemaValue.OptionVal(SchemaValue.S32(3)),
        SchemaValue.ResultVal(true, null),
        SchemaValue.ResultVal(false, SchemaValue.Str("bad")),
        SchemaValue.VariantVal(1, null),
        SchemaValue.VariantVal(2, SchemaValue.S32(9)),
        SchemaValue.EnumVal(2),
        SchemaValue.FlagsVal(listOf(true, false)),
        SchemaValue.MapVal(listOf(SchemaValue.Str("k") to SchemaValue.U32(1u))),
        SchemaValue.TextVal("t", "en"),
        SchemaValue.TextVal("t", null),
        SchemaValue.BinaryVal(listOf(0u, 255u), "image/png"),
        SchemaValue.BinaryVal(emptyList(), null),
        SchemaValue.PathVal("/a/b"),
        SchemaValue.UrlVal("https://example.invalid/x"),
        SchemaValue.UuidVal(0x0123456789abcdefL, -0x1L),
        SchemaValue.DatetimeVal(1_700_000_000L, 123_000_000),
        SchemaValue.DurationVal(-5L),
        SchemaValue.QuantityVal(-12L, 2, "m"),
        SchemaValue.UnionVal("tag", SchemaValue.Str("body")),
    )

    @Test
    fun every_kind_round_trips() {
        for (value in sample()) {
            val rendered = SchemaValueJson.render(value)
            assertEquals(value, SchemaValueJson.fromJson(Json.parse(rendered)), rendered)
        }
    }

    @Test
    fun the_wire_form_matches_the_server_shape() {
        assertEquals("""{"kind":"s64","value":"-9223372036854775808"}""", SchemaValueJson.render(SchemaValue.S64(Long.MIN_VALUE)))
        assertEquals("""{"kind":"u64","value":"18446744073709551615"}""", SchemaValueJson.render(SchemaValue.U64(ULong.MAX_VALUE)))
        assertEquals("""{"kind":"u32","value":4294967295}""", SchemaValueJson.render(SchemaValue.U32(UInt.MAX_VALUE)))
        assertEquals(
            """{"kind":"record","value":{"fields":[{"kind":"s32","value":1},{"kind":"string","value":"a"}]}}""",
            SchemaValueJson.render(SchemaValue.Record(listOf(SchemaValue.S32(1), SchemaValue.Str("a")))),
        )
        assertEquals(
            """{"kind":"variant","value":{"case":2,"payload":{"kind":"s32","value":9}}}""",
            SchemaValueJson.render(SchemaValue.VariantVal(2, SchemaValue.S32(9))),
        )
        assertEquals("""{"kind":"variant","value":{"case":1}}""", SchemaValueJson.render(SchemaValue.VariantVal(1, null)))
        assertEquals("""{"kind":"option","value":{"inner":null}}""", SchemaValueJson.render(SchemaValue.OptionVal(null)))
        assertEquals(
            """{"kind":"result","value":{"tag":"err","value":{"kind":"string","value":"bad"}}}""",
            SchemaValueJson.render(SchemaValue.ResultVal(false, SchemaValue.Str("bad"))),
        )
        assertEquals("""{"kind":"text","value":{"language":"en","text":"t"}}""", SchemaValueJson.render(SchemaValue.TextVal("t", "en")))
        assertEquals(
            """{"kind":"binary","value":{"bytes":[0,255],"mimeType":"image/png"}}""",
            SchemaValueJson.render(SchemaValue.BinaryVal(listOf(0u, 255u), "image/png")),
        )
        assertEquals(
            """{"kind":"uuid","value":"01234567-89ab-cdef-ffff-ffffffffffff"}""",
            SchemaValueJson.render(SchemaValue.UuidVal(0x0123456789abcdefL, -1L)),
        )
        assertEquals(
            """{"kind":"datetime","value":{"value":"2023-11-14T22:13:20.123Z"}}""",
            SchemaValueJson.render(SchemaValue.DatetimeVal(1_700_000_000L, 123_000_000)),
        )
        assertEquals("""{"kind":"duration","value":{"nanoseconds":"-5"}}""", SchemaValueJson.render(SchemaValue.DurationVal(-5L)))
        assertEquals("""{"kind":"fixed-list","value":{"elements":[{"kind":"s8","value":1}]}}""", SchemaValueJson.render(SchemaValue.FixedListVal(listOf(SchemaValue.S8(1)))))
        assertEquals("""{"kind":"map","value":{"entries":[[{"kind":"string","value":"k"},{"kind":"u32","value":1}]]}}""", SchemaValueJson.render(SchemaValue.MapVal(listOf(SchemaValue.Str("k") to SchemaValue.U32(1u)))))
    }

    @Test
    fun exceptional_floats_use_the_float_marker_and_negative_zero_survives() {
        assertEquals("""{"kind":"f64","value":{"${'$'}float":"nan"}}""", SchemaValueJson.render(SchemaValue.F64(Double.NaN)))
        assertEquals(
            """{"kind":"f32","value":{"${'$'}float":"negative-infinity"}}""",
            SchemaValueJson.render(SchemaValue.F32(Float.NEGATIVE_INFINITY)),
        )
        val negativeZero = SchemaValueJson.fromJson(Json.parse("""{"kind":"f64","value":-0}""")) as SchemaValue.F64
        assertTrue(1.0 / negativeZero.v < 0, "negative zero was lost")
        assertTrue((SchemaValueJson.fromJson(Json.parse("""{"kind":"f64","value":{"${'$'}float":"nan"}}""")) as SchemaValue.F64).v.isNaN())
    }

    @Test
    fun values_out_of_range_or_not_canonical_are_rejected() {
        val bad = listOf(
            """{"kind":"s8","value":128}""",
            """{"kind":"u8","value":-1}""",
            """{"kind":"u16","value":65536}""",
            """{"kind":"u32","value":4294967296}""",
            """{"kind":"s32","value":1.5}""",
            """{"kind":"s64","value":"09"}""",
            """{"kind":"u64","value":"-1"}""",
            """{"kind":"u64","value":"18446744073709551616"}""",
            """{"kind":"duration","value":{"nanoseconds":"1e3"}}""",
            """{"kind":"enum","value":{"case":-1}}""",
            """{"kind":"nope","value":1}""",
            """{"kind":"s32"}""",
            """{"kind":"s32","value":1,"extra":2}""",
            """{"kind":"record","value":{"fields":[],"extra":1}}""",
            """{"kind":"result","value":{"tag":"maybe","value":null}}""",
            """{"kind":"char","value":"ab"}""",
            """{"kind":"char","value":""}""",
            """{"kind":"uuid","value":"not-a-uuid"}""",
            """{"kind":"map","value":{"entries":[[{"kind":"s32","value":1}]]}}""",
        )
        for (json in bad) {
            assertFailsWith<BridgeException>("accepted $json") { SchemaValueJson.fromJson(Json.parse(json)) }
        }
    }

    @Test
    fun a_literal_that_is_huge_only_in_its_exponent_is_rejected_quickly() {
        for (json in listOf("""{"kind":"s32","value":1e999999999}""", """{"kind":"u64","value":1E+400}""")) {
            assertFailsWith<BridgeException>("accepted $json") { SchemaValueJson.fromJson(Json.parse(json)) }
        }
        assertEquals(SchemaValue.S32(100), SchemaValueJson.fromJson(Json.parse("""{"kind":"s32","value":1e2}""")))
    }

    @Test
    fun a_character_outside_the_basic_multilingual_plane_is_rejected_on_decode() {
        val error = assertFailsWith<BridgeException> { SchemaValueJson.fromJson(Json.parse("{\"kind\":\"char\",\"value\":\"\uD83D\uDE00\"}")) }
        assertTrue(error.message!!.contains("Basic Multilingual Plane"), error.message)
    }

    @Test
    fun a_lone_surrogate_is_rejected_on_encode() {
        assertFailsWith<BridgeException> { SchemaValueJson.render(SchemaValue.Chr('\uD800')) }
    }
}

class TypesTest {
    @Test
    fun a_uuid_has_the_canonical_form_and_parses_back() {
        val uuid = Uuid(0x0123456789abcdefL, -0x0123456789abcdf0L)
        assertEquals("01234567-89ab-cdef-fedc-ba9876543210", uuid.toStandardString())
        assertEquals(uuid, Uuid.fromStandardString(uuid.toStandardString().uppercase()))
        assertFailsWith<BridgeException> { Uuid.fromStandardString("0123456789abcdeffedcba9876543210") }
    }

    @Test
    fun a_datetime_converts_to_and_from_iso() {
        val at = Datetime(1_700_000_000L, 5)
        assertEquals("2023-11-14T22:13:20.000000005Z", at.toIsoString())
        assertEquals(at, Datetime.parse(at.toIsoString()))
        assertFailsWith<BridgeException> { Datetime.parse("yesterday") }
        assertFailsWith<BridgeException> { Datetime(Long.MAX_VALUE, 0).toIsoString() }
        assertFailsWith<BridgeException> { SchemaValueJson.render(SchemaValue.DatetimeVal(Long.MIN_VALUE, 0)) }
    }
}

class BridgeProtocolTest {
    private val configuration = Configuration(GolemServer.Local, "my-app", "local")

    @Test
    fun a_create_request_lists_the_members_in_the_server_order() {
        val body = BridgeProtocol.renderCreateAgentRequest(configuration, "Counter", SchemaValue.Record(listOf(SchemaValue.Str("c1"))), null)
        assertEquals(
            """{"appName":"my-app","envName":"local","agentTypeName":"Counter","parameters":{"kind":"record","value":{"fields":[{"kind":"string","value":"c1"}]}},"config":[]}""",
            body,
        )
        val phantom = BridgeProtocol.renderCreateAgentRequest(configuration, "Counter", SchemaValue.Record(emptyList()), "p-1")
        assertTrue(phantom.contains(""","phantomId":"p-1","config":[]}"""), phantom)
    }

    @Test
    fun an_invocation_request_carries_the_method_the_mode_and_the_schedule() {
        val resolved = ResolvedAgent(configuration, "Counter", SchemaValue.Record(emptyList()), "p-1", null)
        val body = BridgeProtocol.renderInvocationRequest(resolved, "add", SchemaValue.Record(listOf(SchemaValue.S32(2))), "schedule", "2030-01-01T00:00:00Z")
        assertTrue(body.startsWith("""{"appName":"my-app","envName":"local","agentTypeName":"Counter","parameters":"""), body)
        assertTrue(body.contains(""""methodName":"add","methodParameters":{"kind":"record","value":{"fields":[{"kind":"s32","value":2}]}},"mode":"schedule","phantomId":"p-1","scheduleAt":"2030-01-01T00:00:00Z"}"""), body)
    }

    @Test
    fun responses_are_decoded() {
        val created = BridgeProtocol.decodeCreateAgentResponse(Json.parse("""{"agentId":{"componentId":"c","agentId":"Counter(\"c1\")"},"componentRevision":3}"""))
        assertEquals(AgentId("c", "Counter(\"c1\")"), created)
        val result = BridgeProtocol.decodeInvocationResult(
            Json.parse("""{"agentId":{"componentId":"c","agentId":"a"},"idempotencyKey":"k","result":{"graph":{"root":{"kind":"s32","value":{}}},"value":{"kind":"s32","value":7}},"componentRevision":3}"""),
        )
        assertEquals(SchemaValue.S32(7), result.result)
        assertEquals("k", result.idempotencyKey)
        assertEquals("3", result.componentRevision)
        val unit = BridgeProtocol.decodeInvocationResult(Json.parse("""{"agentId":{"componentId":"c","agentId":"a"},"idempotencyKey":"k"}"""))
        assertEquals(null, unit.result)
    }
}

class RuntimeRegressionTest {
    private val configuration = Configuration(GolemServer.Custom("http://localhost:9881", "token"), "my-app", "local")

    /** The printer refuses a lone surrogate, so the parser must not produce one. */
    @Test
    fun a_lone_surrogate_escape_is_refused_on_the_way_in() {
        assertFailsWith<BridgeException> { Json.parse("\"\\ud800\"") }
        assertFailsWith<BridgeException> { Json.parse("\"\\udc00\"") }
        assertFailsWith<BridgeException> { Json.parse("\"\\ud800x\"") }
        // A proper pair still decodes, and still round-trips through the printer.
        val pair = Json.asString(Json.parse("\"\\ud83d\\ude00\""))
        assertEquals("\uD83D\uDE00", pair)
        assertEquals(pair, Json.asString(Json.parse(Json.string(pair).render())))
    }

    /** `Instant.toString` prints a year outside 0000..9999 in an expanded form that RFC 3339 does not admit. */
    @Test
    fun a_datetime_outside_rfc_3339_is_refused_rather_than_sent() {
        assertEquals("2023-11-14T22:13:20Z", Datetime(1700000000, 0).toIsoString())
        val error = assertFailsWith<BridgeException> { Datetime(253402300800, 0).toIsoString() }
        assertTrue(error.message!!.contains("RFC 3339"), error.message!!)
    }

    /** A token the HTTP client rejects is a bad token, not a bad URL. */
    @Test
    fun a_token_that_cannot_be_a_header_is_not_reported_as_a_bad_url() {
        val bad = Configuration(GolemServer.Custom("http://localhost:9881", "tok\nen"), "my-app", "local")
        val error = assertFailsWith<BridgeException> {
            Bridge.createAgent(bad, "Counter", SchemaValue.Record(emptyList()), null)
        }
        assertTrue(error.message!!.contains("server token"), error.message!!)
        assertTrue(!error.message!!.contains("Invalid server URL"), error.message!!)
    }

    /** An `Async` call reports a render failure through the future, not by throwing. */
    @Test
    fun an_async_call_reports_an_unencodable_value_through_the_future() {
        val lone = SchemaValue.Record(listOf(SchemaValue.Str("\uD800")))
        val future = Bridge.createAgentAsync(configuration, "Counter", lone, null)
        assertTrue(future.isCompletedExceptionally)
    }
}
