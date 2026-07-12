package cloud.golem.runtime

import cloud.golem.Principal
import cloud.golem.Uuid
import kotlin.test.Test
import kotlin.test.assertEquals

class PrincipalBytesRoundTripTest {
    private fun rt(p: Principal) = assertEquals(p, PrincipalBytes.decode(PrincipalBytes.encode(p)), "principal round-trip: $p")

    @Test fun anonymous() = rt(Principal.Anonymous)

    @Test fun agent() = rt(Principal.Agent("agent-123"))

    @Test fun golemUser() = rt(Principal.GolemUser(Uuid(0x0123456789ABCDEFuL, 0xFEDCBA9876543210uL)))

    @Test fun oidcFull() = rt(
        Principal.Oidc("s", "iss", "e@x", "n", true, "g", "f", "p", "u", "{}"),
    )

    @Test fun oidcNulls() = rt(
        Principal.Oidc("s", "iss", null, null, null, null, null, null, null, ""),
    )
}
