package p17

import cloud.golem.BaseAgent
import cloud.golem.annotations.Agent
import cloud.golem.annotations.Endpoint
import cloud.golem.annotations.RemoteAgent
import cloud.golem.permissions.Bounds
import cloud.golem.permissions.Card
import cloud.golem.permissions.Holder
import cloud.golem.permissions.Lifespan
import cloud.golem.permissions.PatternGrant
import cloud.golem.permissions.PermissionCard
import cloud.golem.permissions.PermissionError
import cloud.golem.permissions.Permissions
import cloud.golem.permissions.PersistentCard
import cloud.golem.permissions.PolymorphicPermissionCard
import cloud.golem.permissions.ScopeCard
import cloud.golem.permissions.WalletCard
import cloud.golem.runtime.AgentId
import cloud.golem.runtime.Either
import cloud.golem.runtime.HostApi
import cloud.golem.runtime.RpcResult
import cloud.golem.runtime.SchemaValue
import cloud.golem.runtime.WasmRpc
import kotlin.time.Duration.Companion.hours

/** A Kotlin port of the Scala permission-card fixture (same agent types and methods) for the shared harness scenario. */
@RemoteAgent(typeName = "ScalaPermissionCardTarget")
interface CardTarget {
    fun selfAgentId(): String
    fun inspectAllowed(): Boolean
    fun expiringCardCount(): Int
    fun deriveFromWalletError(): String
    fun walletKinds(): String
    fun lentScopeExpiry(): Long?
    fun receiveTemplate(card: PolymorphicPermissionCard): List<String>
    fun receiveConcrete(card: PermissionCard): Boolean
}

private fun kind(entry: WalletCard): String = when (entry) {
    is WalletCard.Concrete -> "concrete"
    is WalletCard.Template -> "template"
    is WalletCard.Scope -> "scope"
}

private fun <T> unwrap(step: String, result: Either<PermissionError, T>): T = when (result) {
    is Either.Right -> result.value
    is Either.Left -> throw IllegalStateException("$step failed: ${result.value}")
}

private fun <T> isRight(r: Either<PermissionError, T>) = r is Either.Right

private fun <T> show(r: Either<PermissionError, T>): String = when (r) {
    is Either.Right -> "Right(${r.value})"
    is Either.Left -> "Left(${r.value::class.simpleName})"
}

private fun optLong(v: Long?): String = if (v == null) "None" else "Some($v)"

private fun isInspectGrant(g: PatternGrant) = g.className == "card" && g.verb == "inspect" && g.resourceId == "*"

@Agent(mount = "/card-target/{name}", description = "Observes from the callee side")
class ScalaPermissionCardTarget(val name: String) : BaseAgent() {
    private fun <A> withWallet(f: (List<WalletCard>) -> A): A {
        val cards = Permissions.wallet()
        try {
            return f(cards)
        } finally {
            cards.forEach { it.card.release() }
        }
    }

    @Endpoint(post = "/self-agent-id")
    fun selfAgentId(): String = HostApi.getSelfMetadata().agentId.agentId

    @Endpoint(post = "/inspect-allowed")
    fun inspectAllowed(): Boolean = withWallet { w -> w.any { isRight(it.card.inspect()) } }

    @Endpoint(post = "/expiring-card-count")
    fun expiringCardCount(): Int = withWallet { w -> w.count { it.card.expiresAt != null } }

    @Endpoint(post = "/wallet-kinds")
    fun walletKinds(): String = withWallet { w -> w.map { kind(it) }.sorted().joinToString(",") }

    @Endpoint(post = "/lent-scope-expiry")
    fun lentScopeExpiry(): Long? = withWallet { w -> w.filterIsInstance<WalletCard.Scope>().firstOrNull()?.card?.expiresAt?.seconds }

    @Endpoint(post = "/receive-template")
    fun receiveTemplate(card: PolymorphicPermissionCard): List<String> {
        val observed = listOf("received-parents=${card.parents.size}", "received-inspect=${isRight(card.inspect())}")
        card.release()
        return observed
    }

    @Endpoint(post = "/receive-concrete")
    fun receiveConcrete(card: PermissionCard): Boolean {
        card.release()
        return true
    }

    @Endpoint(post = "/derive-from-wallet-error")
    fun deriveFromWalletError(): String {
        val grant = PatternGrant("card", "*", "*", "inspect", "*")
        return when (val r = Permissions.deriveFromWallet(Bounds(lowerPositive = listOf(grant)))) {
            is Either.Left -> r.value::class.simpleName ?: "?"
            is Either.Right -> {
                r.value.release()
                "derived"
            }
        }
    }
}

@Agent(mount = "/card-agent/{name}", description = "Runs the shared permission-card scenario")
class ScalaPermissionCardAgent(val name: String) : BaseAgent() {
    private var keptScope: ScopeCard? = null

    private fun remote(target: String) = CardTargetRpc(SchemaValue.Record(listOf(SchemaValue.Str(target))))

    private fun walletHas(id: cloud.golem.permissions.CardId): Boolean {
        val cards = Permissions.wallet()
        try {
            return cards.any { it.card.id == id }
        } finally {
            cards.forEach { it.card.release() }
        }
    }

    /** The wallet entry that grants `card:inspect`; every other entry is released. */
    private fun rootEntry(): WalletCard {
        val wallet = Permissions.wallet()
        val roots = wallet.filter { e -> (e.card.inspect() as? Either.Right)?.value?.lowerPositive?.any { isInspectGrant(it) } == true }
        wallet.filter { it !in roots }.forEach { it.card.release() }
        roots.drop(1).forEach { it.card.release() }
        return roots.firstOrNull() ?: throw IllegalStateException("no card:inspect card in the wallet")
    }

    @Endpoint(post = "/stale-scope")
    fun staleScope(target: String): List<String> {
        val scope = keptScope ?: throw IllegalStateException("run the scenario first")
        val parent = Permissions.deriveScope(listOf(scope), Bounds.empty)
        val client = remote(target)
        val attach = try {
            "attach=${client.withScopeCard(scope) { inspectAllowed() }}"
        } catch (e: Exception) {
            "attach-failed=${(e.message ?: "").contains("denied")}"
        }
        client.close()
        (parent as? Either.Right)?.value?.release()
        scope.release()
        keptScope = null
        return listOf(attach, "stale-parent=${show(parent).let { if (parent is Either.Right) "Right" else it }}", "released=${!scope.isPresent}")
    }

    @Endpoint(post = "/send-template")
    fun sendTemplate(target: String): List<String> {
        val entry = rootEntry()
        if (entry !is WalletCard.Template) {
            entry.card.release()
            return listOf("root-kind=${kind(entry)}")
        }
        val template = entry.card
        val client = remote(target)
        val received = client.receiveTemplate(template)
        val kinds = client.walletKinds()
        client.close()
        return received + listOf("sent-moved=${!template.isPresent}", "receiver-kinds=$kinds")
    }

    @Endpoint(post = "/scenario")
    fun scenario(target: String): List<String> {
        val entry = rootEntry()
        val rootKind = kind(entry)
        val root = entry.card
        val inspectGrant = unwrap("inspect root", root.inspect()).lowerPositive.firstOrNull { isInspectGrant(it) }
            ?: throw IllegalStateException("the root card does not grant card:inspect")
        val inspectOnly = Bounds(lowerPositive = listOf(inspectGrant))

        // 1. Derive a card from the wallet. 2. Inspect it.
        val derived: PersistentCard = unwrap("deriveFromWallet", Permissions.deriveFromWallet(inspectOnly))
        val derivedView = unwrap("inspect derived", derived.inspect())

        // 3. Derive a child with a lifespan.
        val child = unwrap("derive child", derived.derive(inspectOnly, Lifespan.of(1.hours)))
        val childParent = child is PersistentCard.Concrete && child.card.parents == listOf(derived.id)
        val childExpiry = child.expiresAt != null && derived.expiresAt == null

        val template = (entry as? WalletCard.Template)?.card ?: throw IllegalStateException("the root card is ${kind(entry)}")
        val self = HostApi.getSelfMetadata()
        val selfHolder = Holder.Agent(self.agentId)
        val timed = listOf(2.hours, 1.hours, 3.hours).map { unwrap("derive $it child", template.derive(inspectOnly, Lifespan.of(it))) }
        val templateDerived = timed.all { it.parents == listOf(template.id) }
        val timedIds = timed.map { it.id }
        val shortExpiry = timed[1].expiresAt
        timed.forEach { unwrap("install timed child", it.install(selfHolder)) }
        val (children, rest) = Permissions.wallet().partition { it.card.id in timedIds }
        rest.forEach { it.card.release() }
        val childKinds = children.map { kind(it) }.sorted().joinToString(",")
        val timedRoots: List<Card> = children.sortedBy { timedIds.indexOf(it.card.id) }.map { it.card }

        // 4. Derive a scope card and send it with awaited RPC calls.
        val scope = unwrap("deriveScope", Permissions.deriveScope(listOf(root), inspectOnly))
        val client = remote(target)
        val expiringScope = unwrap("deriveScope expiring", Permissions.deriveScope(listOf(root) + timedRoots, inspectOnly))
        val scopeExpiry = shortExpiry != null && expiringScope.expiresAt == shortExpiry

        val targetAgentId = client.selfAgentId()
        val before = client.inspectAllowed()
        val perCall = client.withScopeCard(scope) { inspectAllowed() }
        val defaultClient = remote(target).withDefaultScopeCard(scope)
        val byDefault = defaultClient.inspectAllowed()
        defaultClient.close()
        val cancelable = run {
            val raw = WasmRpc("ScalaPermissionCardTarget", SchemaValue.Record(listOf(SchemaValue.Str(target))))
            val future = raw.asyncInvokeAndAwait("inspectAllowed", SchemaValue.Record(emptyList()), "bool", scope)
            val result = future.get()
            future.close()
            raw.close()
            ((result as? RpcResult.Ok)?.value as? SchemaValue.Bool)?.v
        }
        val after = client.inspectAllowed()
        val scopeUsable = scope.isPresent && isRight(scope.inspect())
        val deniedDerive = client.deriveFromWalletError()
        val kindsPlain = client.walletKinds()
        val kindsLent = client.withScopeCard(scope) { walletKinds() }
        val lentExpiry = client.withScopeCard(expiringScope) { lentScopeExpiry() }
        val lentNoExpiry = client.withScopeCard(scope) { lentScopeExpiry() }

        // 5. Install a card on another agent.
        val holder = Holder.Agent(AgentId(self.agentId.componentId, targetAgentId))
        val installed = child.install(holder)
        val targetExpiring = client.expiringCardCount()
        val targetInspect = client.inspectAllowed()

        // 6. Revoke a card and check the returned count and the lost authority.
        val selfHeld = unwrap("derive self-held", derived.derive(inspectOnly))
        val grandchild = unwrap("derive grandchild", selfHeld.derive(inspectOnly))
        val selfHeldId = selfHeld.id
        val selfInstalled = selfHeld.install(selfHolder)
        val heldBefore = walletHas(selfHeldId)
        val (mine, others) = Permissions.wallet().partition { it.card.id == selfHeldId }
        others.forEach { it.card.release() }
        mine.drop(1).forEach { it.card.release() }
        val selfKind = mine.firstOrNull()?.let { "Some(${kind(it)})" } ?: "None"
        val revoked = mine.firstOrNull()?.let {
            when (it) {
                is WalletCard.Concrete -> "Some(${show(it.card.revoke())})"
                else -> {
                    it.card.release()
                    "Some(Left(NotPermitted))"
                }
            }
        } ?: "None"
        val heldAfter = walletHas(selfHeldId)

        // Revoking the parent of the card installed on the target removes the target's authority.
        val parentRevoked = derived.revoke()
        val targetInspectAfter = client.inspectAllowed()
        val targetExpiringAfter = client.expiringCardCount()

        // 7. Negative cases.
        val movedChild = try {
            child.inspect()
            false
        } catch (e: IllegalStateException) {
            true
        }
        val mismatchCard = unwrap("derive mismatch card", Permissions.deriveFromWallet(Bounds(lowerPositive = listOf(inspectGrant.copy(recipient = "nobody@invalid.example")))))
        val mismatch = mismatchCard.install(holder)
        client.close()

        val childrenRevoked = children.map {
            when (it) {
                is WalletCard.Concrete -> show(it.card.revoke())
                else -> {
                    it.card.release()
                    "Left(NotPermitted)"
                }
            }
        }
        derived.release()
        root.release()
        expiringScope.release()
        keptScope = scope
        grandchild.release()
        return listOf(
            "root-kind=$rootKind",
            "derived-inspect=${derivedView.lowerPositive == listOf(inspectGrant)}",
            "child-parent=$childParent",
            "child-expiry=$childExpiry",
            "template-derive=$templateDerived",
            "template-children=$childKinds",
            "scope-expiry=$scopeExpiry",
            "root-scope-expiry=${scope.expiresAt?.let { "Some($it)" } ?: "None"}",
            "before=$before",
            "per-call=$perCall",
            "default=$byDefault",
            "cancelable=$cancelable",
            "after=$after",
            "scope-usable=$scopeUsable",
            "denied-derive=$deniedDerive",
            "kinds-plain=$kindsPlain",
            "kinds-lent=$kindsLent",
            "lent-expiry=${lentExpiry != null && lentExpiry == shortExpiry?.seconds}",
            "lent-no-expiry=${optLong(lentNoExpiry)}",
            "installed=${isRight(installed)}",
            "target-expiring=$targetExpiring",
            "target-inspect=$targetInspect",
            "self-installed=${isRight(selfInstalled)}",
            "held-before=$heldBefore",
            "self-held-kind=$selfKind",
            "revoked=$revoked",
            "held-after=$heldAfter",
            "parent-revoked=${show(parentRevoked)}",
            "target-inspect-after-revoke=$targetInspectAfter",
            "target-expiring-after-revoke=$targetExpiringAfter",
            "moved-child=$movedChild",
            "mismatch=${show(mismatch).replace("Right(kotlin.Unit)", "Right(())")}",
            "children-revoked=List(${childrenRevoked.joinToString(", ")})",
        )
    }
}
