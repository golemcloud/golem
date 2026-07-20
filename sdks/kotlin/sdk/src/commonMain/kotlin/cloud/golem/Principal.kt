package cloud.golem

/**
 * The authenticated identity of whoever invoked the current agent method — delivered by the Golem
 * host on every `initialize`/`invoke` call. Read it inside an agent via [BaseAgent.principal].
 *
 * Mirrors `golem:agent/common`'s `principal` variant. [Anonymous] means the call carried no
 * authenticated identity.
 */
sealed class Principal {

    /** An OpenID Connect identity (a human or service authenticated via OIDC). */
    data class Oidc(
        /** The `sub` claim — the subject's stable identifier. */
        val sub: String,
        /** The token issuer (`iss`). */
        val issuer: String,
        val email: String?,
        val name: String?,
        val emailVerified: Boolean?,
        val givenName: String?,
        val familyName: String?,
        val picture: String?,
        val preferredUsername: String?,
        /** The raw claims blob (JSON) as provided by the issuer. */
        val claims: String,
    ) : Principal()

    /** Another Golem agent, identified by its canonical string agent-id. */
    data class Agent(val agentId: String) : Principal()

    /** A Golem user account, identified by its account UUID. */
    data class GolemUser(val accountId: Uuid) : Principal()

    /** No authenticated caller. */
    object Anonymous : Principal()
}
