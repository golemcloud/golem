package cloud.golem.runtime

import cloud.golem.Principal
import cloud.golem.Uuid
import cloud.golem.wasm.alloc
import cloud.golem.wasm.storeByte
import cloud.golem.wasm.storeLong
import cloud.golem.wasm.writeStringField
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertIs

/**
 * Decodes hand-built, host-shaped `principal` variant buffers (the identity the host passes to
 * initialize/invoke) via [liftPrincipal], at the abi-dump-verified offsets: variant tag@0,
 * payload@8; oidc-principal fields at their own offsets; agent-id string @16 within agent-id;
 * account-id uuid at payload. Pure linear-memory ops (no host WIT import), so they run under the
 * plain wasmWasi nodejs runner.
 */
class PrincipalDecodeTest {

    private fun buf() = alloc(112, 8)

    @Test
    fun anonymous() {
        val b = buf()
        storeByte(b, 3) // tag = anonymous
        assertEquals(Principal.Anonymous, liftPrincipal(b))
    }

    @Test
    fun agent_carries_agent_id_string() {
        val b = buf()
        storeByte(b, 1) // tag = agent
        // agent-principal { agent-id } @8; agent-id { component-id @0 (16B), agent-id: string @16 }.
        // string field lives at b + 8 (payload) + 16 = b+24.
        writeStringField(b, 24, "example:counter/CounterAgent(\"c1\")")
        val p = assertIs<Principal.Agent>(liftPrincipal(b))
        assertEquals("example:counter/CounterAgent(\"c1\")", p.agentId)
    }

    @Test
    fun golem_user_carries_account_uuid() {
        val b = buf()
        storeByte(b, 2) // tag = golem-user
        storeLong(b + 8, 0x0123456789ABCDEFL) // account-id.uuid.high-bits @ payload+0
        storeLong(b + 16, 0x76543210FEDCBA98L) // low-bits @ payload+8
        val p = assertIs<Principal.GolemUser>(liftPrincipal(b))
        assertEquals(Uuid(0x0123456789ABCDEFuL, 0x76543210FEDCBA98uL), p.accountId)
    }

    @Test
    fun oidc_decodes_strings_options_and_email_verified() {
        val b = buf()
        storeByte(b, 0) // tag = oidc; oidc-principal payload @ b+8
        val o = b + 8
        writeStringField(o, 0, "user-123") // sub @0
        writeStringField(o, 8, "https://issuer") // issuer @8
        storeByte(o + 16, 1)
        writeStringField(o, 20, "u@example.com") // email @16 = some
        storeByte(o + 28, 0) // name @28 = none
        storeByte(o + 40, 1)
        storeByte(o + 41, 1) // email-verified @40 = some(true)
        storeByte(o + 44, 0) // given-name none
        storeByte(o + 56, 0) // family-name none
        storeByte(o + 68, 0) // picture none
        storeByte(o + 80, 0) // preferred-username none
        writeStringField(o, 92, "{\"role\":\"admin\"}") // claims @92

        val p = assertIs<Principal.Oidc>(liftPrincipal(b))
        assertEquals("user-123", p.sub)
        assertEquals("https://issuer", p.issuer)
        assertEquals("u@example.com", p.email)
        assertEquals(null, p.name)
        assertEquals(true, p.emailVerified)
        assertEquals(null, p.givenName)
        assertEquals("{\"role\":\"admin\"}", p.claims)
    }
}
