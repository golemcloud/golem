package cloud.golem.runtime.host

import cloud.golem.wasm.alloc
import cloud.golem.wasm.storeByte
import cloud.golem.wasm.storeInt
import cloud.golem.wasm.writeStringField
import kotlin.test.Test
import kotlin.test.assertEquals

/**
 * Decodes hand-built, host-shaped `secret-error` buffers (the err arm of `reveal`'s
 * `result<schema-value-tree, secret-error>`) via [liftSecretError], at the abi-dump-verified
 * offsets (tag@0, string/list payload ptr@4/len@8). Pure linear-memory ops (no host WIT import),
 * so they run under the plain wasmWasi nodejs runner. The reveal call path itself is
 * compile-verified; the request-side schema-graph build is covered by the schema-graph/typed-
 * schema-value round-trip tests.
 */
class SecretErrorDecodeTest {

    @Test
    fun unavailable_carries_message() {
        val b = alloc(12, 4)
        storeByte(b, 0) // tag = unavailable
        writeStringField(b, 4, "store gone") // string @ b+4 (ptr) / b+8 (len)
        assertEquals(SecretError.Unavailable("store gone"), liftSecretError(b))
    }

    @Test
    fun internal_carries_message() {
        val b = alloc(12, 4)
        storeByte(b, 2) // tag = internal
        writeStringField(b, 4, "boom")
        assertEquals(SecretError.Internal("boom"), liftSecretError(b))
    }

    @Test
    fun version_not_found_carries_bytes() {
        val bytes = alloc(3, 1)
        storeByte(bytes, 1)
        storeByte(bytes + 1, 2)
        storeByte(bytes + 2, 3)
        val b = alloc(12, 4)
        storeByte(b, 1) // tag = version-not-found
        storeInt(b + 4, bytes) // secret-version.bytes list: ptr@b+4
        storeInt(b + 8, 3) // len@b+8
        assertEquals(SecretError.VersionNotFound(listOf<UByte>(1u, 2u, 3u)), liftSecretError(b))
    }
}
