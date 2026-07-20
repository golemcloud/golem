package cloud.golem.runtime

import cloud.golem.Principal
import cloud.golem.Uuid

/**
 * Self-contained tagged binary codec for [Principal]. Independent of the host's principal variant
 * record ([liftPrincipal] reads that from a live pointer); snapshots control both ends, so this
 * uses a compact, dependency-free framing. Format v1:
 *   tag: u8 (0=Oidc, 1=Agent, 2=GolemUser, 3=Anonymous)
 *   s := [len: u32 BE][utf8];  opt-string := [present: u8][s?];  opt-bool := [present: u8][u8?]
 *   Oidc := 0, s(sub), s(issuer), opt-string(email), opt-string(name), opt-bool(emailVerified),
 *           opt-string(givenName), opt-string(familyName), opt-string(picture),
 *           opt-string(preferredUsername), s(claims)
 *   Agent := 1, s(agentId);  GolemUser := 2, u64 BE(high), u64 BE(low);  Anonymous := 3
 */
internal object PrincipalBytes {
    fun encode(p: Principal): ByteArray {
        val out = ArrayList<Byte>(32)
        when (p) {
            is Principal.Oidc -> {
                out.add(0)
                putStr(out, p.sub)
                putStr(out, p.issuer)
                putOptStr(out, p.email)
                putOptStr(out, p.name)
                putOptBool(out, p.emailVerified)
                putOptStr(out, p.givenName)
                putOptStr(out, p.familyName)
                putOptStr(out, p.picture)
                putOptStr(out, p.preferredUsername)
                putStr(out, p.claims)
            }
            is Principal.Agent -> {
                out.add(1)
                putStr(out, p.agentId)
            }
            is Principal.GolemUser -> {
                out.add(2)
                putU64(out, p.accountId.highBits)
                putU64(out, p.accountId.lowBits)
            }
            Principal.Anonymous -> out.add(3)
        }
        return out.toByteArray()
    }

    fun decode(bytes: ByteArray): Principal {
        val r = Reader(bytes)
        return when (val tag = r.u8()) {
            0 -> Principal.Oidc(r.str(), r.str(), r.optStr(), r.optStr(), r.optBool(), r.optStr(), r.optStr(), r.optStr(), r.optStr(), r.str())
            1 -> Principal.Agent(r.str())
            2 -> Principal.GolemUser(Uuid(r.u64(), r.u64()))
            3 -> Principal.Anonymous
            else -> error("PrincipalBytes.decode: unknown tag $tag")
        }
    }

    private fun putStr(out: ArrayList<Byte>, s: String) {
        val b = s.encodeToByteArray()
        putU32(out, b.size)
        b.forEach { out.add(it) }
    }
    private fun putOptStr(out: ArrayList<Byte>, s: String?) {
        if (s == null) {
            out.add(0)
        } else {
            out.add(1)
            putStr(out, s)
        }
    }
    private fun putOptBool(out: ArrayList<Byte>, b: Boolean?) {
        if (b == null) {
            out.add(0)
        } else {
            out.add(1)
            out.add(if (b) 1 else 0)
        }
    }
    private fun putU32(out: ArrayList<Byte>, v: Int) {
        out.add((v ushr 24).toByte())
        out.add((v ushr 16).toByte())
        out.add((v ushr 8).toByte())
        out.add(v.toByte())
    }
    private fun putU64(out: ArrayList<Byte>, v: ULong) {
        for (s in 56 downTo 0 step 8) out.add((v shr s).toByte())
    }

    private class Reader(val b: ByteArray) {
        var i = 0
        fun u8(): Int = b[i++].toInt() and 0xFF
        fun u32(): Int {
            var v = 0
            repeat(4) { v = (v shl 8) or u8() }
            return v
        }
        fun u64(): ULong {
            var v = 0uL
            repeat(8) { v = (v shl 8) or u8().toULong() }
            return v
        }
        fun str(): String {
            val n = u32()
            val s = b.decodeToString(i, i + n)
            i += n
            return s
        }
        fun optStr(): String? = if (u8() == 0) null else str()
        fun optBool(): Boolean? = if (u8() == 0) null else (u8() != 0)
    }
}
