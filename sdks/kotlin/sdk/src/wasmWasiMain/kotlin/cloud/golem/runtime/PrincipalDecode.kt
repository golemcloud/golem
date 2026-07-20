@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class)

package cloud.golem.runtime

import cloud.golem.Principal
import cloud.golem.Uuid
import cloud.golem.wasm.liftString
import cloud.golem.wasm.loadByte
import cloud.golem.wasm.loadInt
import cloud.golem.wasm.loadLong

// Decodes the `principal` variant (golem:agent/common@2.0.0) the host passes to initialize/invoke.
// All offsets verified via abi-dump. principal: size=112 align=8, tag@0, payload@8. Cases:
//   0 oidc(oidc-principal), 1 agent(agent-principal), 2 golem-user(golem-user-principal), 3 anonymous.

/** Reads a canonical-ABI `string` (ptr@off, len@off+4) at [base]+[off]. */
private fun str(base: Int, off: Int): String = liftString(loadInt(base + off), loadInt(base + off + 4))

/** Reads `option<string>` (tag@off, string@off+4) at [base]+[off]. */
private fun optStr(base: Int, off: Int): String? = if (loadByte(base + off).toInt() and 0xFF == 0) null else str(base, off + 4)

// oidc-principal: size=100 align=4. sub@0, issuer@8, email@16 (opt), name@28 (opt),
// email-verified@40 (opt<bool>: tag@40,bool@41), given-name@44 (opt), family-name@56 (opt),
// picture@68 (opt), preferred-username@80 (opt), claims@92.
private fun liftOidcPrincipal(b: Int): Principal.Oidc = Principal.Oidc(
    sub = str(b, 0),
    issuer = str(b, 8),
    email = optStr(b, 16),
    name = optStr(b, 28),
    emailVerified = if (loadByte(b + 40).toInt() and 0xFF == 0) null else (loadByte(b + 41).toInt() != 0),
    givenName = optStr(b, 44),
    familyName = optStr(b, 56),
    picture = optStr(b, 68),
    preferredUsername = optStr(b, 80),
    claims = str(b, 92),
)

// account-id { uuid } (16B); uuid { high-bits: u64 @0, low-bits: u64 @8 }.
private fun liftUuid(b: Int): Uuid = Uuid(loadLong(b).toULong(), loadLong(b + 8).toULong())

/** Lifts the `principal` variant at [base] (the 112-byte record). */
internal fun liftPrincipal(base: Int): Principal {
    val payload = base + 8
    return when (loadByte(base).toInt() and 0xFF) {
        0 -> liftOidcPrincipal(payload)
        // agent-principal { agent-id } @0; agent-id { component-id @0 (16B), agent-id: string @16 }.
        1 -> Principal.Agent(str(payload, 16))
        // golem-user-principal { account-id } @0; account-id { uuid } @0.
        2 -> Principal.GolemUser(liftUuid(payload))
        else -> Principal.Anonymous
    }
}
