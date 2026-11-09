@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class)

package cloud.golem.runtime.wasi

import cloud.golem.wasm.alloc
import cloud.golem.wasm.storeByte
import cloud.golem.wasm.storeInt
import cloud.golem.wasm.storeLong
import cloud.golem.wasm.storeShort
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertNull

/**
 * `liftHttpErrorCode` decodes `wasi:http/types@0.3.0`'s `error-code` (size=32 align=8, tag@0,
 * payload@8), derived in this plan's Task 2 with `wit_parser::SizeAlign` against the real WIT, not
 * hand-computed. This writes the raw bytes a real host would -- the independent oracle -- and
 * checks the decoder against them directly, the same two-oracle style as
 * `LoweredPathQuantityUnionTest`. [everyTagDecodesToItsOwnCase] enumerates all 39 cases by tag
 * (Task 5's own job); the remaining tests each pin a specific payload's byte-level decoding for
 * the representative cases that exercise a distinct shape (a plain record, an optional scalar, an
 * optional string, and the one `field-size-payload` case that is *not* `option`-wrapped, unlike
 * its three siblings) -- `everyTagDecodesToItsOwnCase` does not re-derive those bytes, since
 * Task 2/3 already did and a second derivation would just be a second chance to copy the same
 * mistake, not an independent check.
 */
class HttpTest {
    private fun errorCodeBuffer(): Int = alloc(HTTP_ERROR_SIZE_FOR_TEST, HTTP_ERROR_ALIGN_FOR_TEST)

    @Test
    fun aNoPayloadCaseDecodesByTagAlone() {
        val base = errorCodeBuffer()
        storeByte(base, 6) // connection-refused
        assertEquals(HttpErrorCode.ConnectionRefused, liftHttpErrorCode(base))
    }

    /**
     * Every one of the 39 WIT `error-code` cases, by tag, matched against the exhaustive `when` in
     * `liftHttpErrorCode`. No-payload cases assert full equality against the expected object;
     * payload-bearing cases assert only that the decoder reaches the right sealed-class case (the
     * byte-level payload decoding itself is covered by this file's other, more specific tests) --
     * written with a minimal, uniform payload (every optional field `None`, every non-optional
     * string field `""`) so each case needs no more setup than a no-payload one, keeping this test
     * a tag-to-case table and not 39 near-duplicates of the payload tests above it.
     */
    @Test
    fun everyTagDecodesToItsOwnCase() {
        val noPayloadCases = mapOf(
            0 to HttpErrorCode.DnsTimeout,
            2 to HttpErrorCode.DestinationNotFound,
            3 to HttpErrorCode.DestinationUnavailable,
            4 to HttpErrorCode.DestinationIpProhibited,
            5 to HttpErrorCode.DestinationIpUnroutable,
            6 to HttpErrorCode.ConnectionRefused,
            7 to HttpErrorCode.ConnectionTerminated,
            8 to HttpErrorCode.ConnectionTimeout,
            9 to HttpErrorCode.ConnectionReadTimeout,
            10 to HttpErrorCode.ConnectionWriteTimeout,
            11 to HttpErrorCode.ConnectionLimitReached,
            12 to HttpErrorCode.TlsProtocolError,
            13 to HttpErrorCode.TlsCertificateError,
            15 to HttpErrorCode.HttpRequestDenied,
            16 to HttpErrorCode.HttpRequestLengthRequired,
            18 to HttpErrorCode.HttpRequestMethodInvalid,
            19 to HttpErrorCode.HttpRequestUriInvalid,
            20 to HttpErrorCode.HttpRequestUriTooLong,
            25 to HttpErrorCode.HttpResponseIncomplete,
            33 to HttpErrorCode.HttpResponseTimeout,
            34 to HttpErrorCode.HttpUpgradeFailed,
            35 to HttpErrorCode.HttpProtocolError,
            36 to HttpErrorCode.LoopDetected,
            37 to HttpErrorCode.ConfigurationError,
        )
        for ((tag, expected) in noPayloadCases) {
            val base = errorCodeBuffer()
            storeByte(base, tag.toByte())
            assertEquals(expected, liftHttpErrorCode(base), "tag $tag")
        }

        // Payload-bearing cases: every optional field None, every non-optional field "" --
        // minimal, not a second byte-level check (see the other tests for that), just confirming
        // the tag routes to the right sealed-class case.
        val payloadCases = listOf(1, 14, 17, 21, 22, 23, 24, 26, 27, 28, 29, 30, 31, 32, 38)
        for (tag in payloadCases) {
            val base = errorCodeBuffer()
            storeByte(base, tag.toByte())
            val payload = base + 8
            when (tag) {
                1 -> { // dns-error: DnsErrorPayload (option<string>, option<u16>), both None
                    storeByte(payload, 0)
                    storeByte(payload + 12, 0)
                }
                14 -> { // tls-alert-received: TlsAlertReceivedPayload (option<u8>, option<string>), both None
                    storeByte(payload, 0)
                    storeByte(payload + 4, 0)
                }
                17, 28 -> storeByte(payload, 0) // option<u64>: None
                21, 23, 26, 29 -> storeByte(payload, 0) // option<u32>: None
                22 -> storeByte(payload, 0) // option<field-size-payload>: None
                24, 27, 30 -> { // field-size-payload direct (not option-wrapped): both fields None
                    storeByte(payload, 0)
                    storeByte(payload + 12, 0)
                }
                31, 32, 38 -> storeByte(payload, 0) // option<string>: None
            }
            val decoded = liftHttpErrorCode(base)
            val expectedClass = when (tag) {
                1 -> HttpErrorCode.DnsError::class
                14 -> HttpErrorCode.TlsAlertReceived::class
                17 -> HttpErrorCode.HttpRequestBodySize::class
                21 -> HttpErrorCode.HttpRequestHeaderSectionSize::class
                22 -> HttpErrorCode.HttpRequestHeaderSize::class
                23 -> HttpErrorCode.HttpRequestTrailerSectionSize::class
                24 -> HttpErrorCode.HttpRequestTrailerSize::class
                26 -> HttpErrorCode.HttpResponseHeaderSectionSize::class
                27 -> HttpErrorCode.HttpResponseHeaderSize::class
                28 -> HttpErrorCode.HttpResponseBodySize::class
                29 -> HttpErrorCode.HttpResponseTrailerSectionSize::class
                30 -> HttpErrorCode.HttpResponseTrailerSize::class
                31 -> HttpErrorCode.HttpResponseTransferCoding::class
                32 -> HttpErrorCode.HttpResponseContentCoding::class
                38 -> HttpErrorCode.InternalError::class
                else -> error("no expected class listed for tag $tag")
            }
            assertEquals(expectedClass, decoded::class, "tag $tag")
        }

        assertEquals(39, noPayloadCases.size + payloadCases.size, "every WIT error-code case must be listed exactly once above")
    }

    @Test
    fun dnsErrorDecodesItsRecordPayload() {
        val base = errorCodeBuffer()
        storeByte(base, 1) // dns-error
        val payload = base + 8
        storeByte(payload, 1) // rcode is Some
        val rcodePtr = writeString("NXDOMAIN")
        storeInt(payload + 4, rcodePtr)
        storeInt(payload + 8, 8)
        storeByte(payload + 12, 1) // info-code is Some
        storeShort(payload + 14, 7)
        val decoded = liftHttpErrorCode(base) as HttpErrorCode.DnsError
        assertEquals("NXDOMAIN", decoded.payload.rcode)
        assertEquals(7, decoded.payload.infoCode)
    }

    @Test
    fun httpRequestBodySizeDecodesAnOptionalU64() {
        val base = errorCodeBuffer()
        storeByte(base, 17) // http-request-body-size
        val payload = base + 8
        storeByte(payload, 1) // Some
        storeLong(payload + 8, 4096L)
        val decoded = liftHttpErrorCode(base) as HttpErrorCode.HttpRequestBodySize
        assertEquals(4096L, decoded.limit)
    }

    @Test
    fun httpRequestBodySizeWithNoneDecodesToNull() {
        val base = errorCodeBuffer()
        storeByte(base, 17)
        storeByte(base + 8, 0) // None
        val decoded = liftHttpErrorCode(base) as HttpErrorCode.HttpRequestBodySize
        assertNull(decoded.limit)
    }

    @Test
    fun internalErrorDecodesAnOptionalString() {
        val base = errorCodeBuffer()
        storeByte(base, 38) // internal-error
        val payload = base + 8
        storeByte(payload, 1)
        val msgPtr = writeString("boom")
        storeInt(payload + 4, msgPtr)
        storeInt(payload + 8, 4)
        val decoded = liftHttpErrorCode(base) as HttpErrorCode.InternalError
        assertEquals("boom", decoded.message)
    }

    @Test
    fun httpRequestTrailerSizeIsNotOptionWrappedUnlikeItsThreeSiblings() {
        // The three siblings (http-response-header-size, http-response-trailer-size, and
        // http-request-trailer-size's own case here) carry `field-size-payload` directly, with no
        // leading option tag -- confirmed against the real WIT with wit_parser (Task 2's ledger
        // entry), not assumed symmetric with http-request-header-size's `option<field-size-payload>`.
        val base = errorCodeBuffer()
        storeByte(base, 24) // http-request-trailer-size
        val payload = base + 8
        storeByte(payload, 0) // field-name: None
        storeByte(payload + 12, 1) // field-size: Some
        storeInt(payload + 16, 64)
        val decoded = liftHttpErrorCode(base) as HttpErrorCode.HttpRequestTrailerSize
        assertNull(decoded.payload.fieldName)
        assertEquals(64, decoded.payload.fieldSize)
    }

    @Test
    fun httpRequestHeaderSizeIsOptionWrapped() {
        val base = errorCodeBuffer()
        storeByte(base, 22) // http-request-header-size
        storeByte(base + 8, 0) // option<field-size-payload>: None
        val decoded = liftHttpErrorCode(base) as HttpErrorCode.HttpRequestHeaderSize
        assertNull(decoded.payload)
    }

    private fun writeString(s: String): Int {
        val bytes = s.encodeToByteArray()
        val ptr = alloc(bytes.size, 1)
        for (i in bytes.indices) storeByte(ptr + i, bytes[i])
        return ptr
    }
}

private const val HTTP_ERROR_SIZE_FOR_TEST = 32
private const val HTTP_ERROR_ALIGN_FOR_TEST = 8
