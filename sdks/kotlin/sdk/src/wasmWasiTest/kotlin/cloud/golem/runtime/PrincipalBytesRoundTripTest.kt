package cloud.golem.runtime

import cloud.golem.Principal
import cloud.golem.Uuid
import kotlin.test.Test
import kotlin.test.assertEquals
import kotlin.test.assertFailsWith

class PrincipalBytesRoundTripTest {
    private fun rt(p: Principal) = assertEquals(p, PrincipalBytes.decode(PrincipalBytes.encode(p)), "principal round-trip: $p")

    @Test fun anonymous() = rt(Principal.Anonymous)

    @Test fun agent() = rt(Principal.Agent(Uuid(3uL, 4uL), "agent-123"))

    @Test fun golemUser() = rt(Principal.GolemUser(Uuid(0x0123456789ABCDEFuL, 0xFEDCBA9876543210uL)))

    @Test fun oidcFull() = rt(
        Principal.Oidc("s", "iss", "e@x", "n", true, "g", "f", "p", "u", "{}"),
    )

    @Test fun oidcNulls() = rt(
        Principal.Oidc("s", "iss", null, null, null, null, null, null, null, ""),
    )

    @Test fun oidcWithEveryOptionalFieldAndEmailVerifiedFalse() = rt(
        Principal.Oidc("subject", "https://issuer", "a@example.test", null, false, null, "Family", null, "login", """{"role":"reader"}"""),
    )

    @Test fun rejectsAnUnknownTag() {
        assertFailsWith<IllegalStateException> { PrincipalBytes.decode(byteArrayOf(9)) }
    }

    @Test fun rejectsMissingOrTruncatedFields() {
        assertFailsWith<IllegalStateException> { PrincipalBytes.decode(ByteArray(0)) }
        assertFailsWith<IllegalStateException> { PrincipalBytes.decode(byteArrayOf(1)) }
        for (p in listOf(Principal.Agent(Uuid(5uL, 6uL), "worker(7)"), Principal.GolemUser(Uuid(1uL, 2uL)), Principal.Oidc("s", "i", "e", null, true, null, null, null, null, "{}"))) {
            val bytes = PrincipalBytes.encode(p)
            assertFailsWith<IllegalStateException>("$p") { PrincipalBytes.decode(bytes.copyOfRange(0, bytes.size - 1)) }
        }
    }

    @Test fun rejectsAStringLengthBeyondTheInput() {
        assertFailsWith<IllegalStateException> { PrincipalBytes.decode(byteArrayOf(1, 0, 0, 0, 9, 'a'.code.toByte())) }
    }
}
