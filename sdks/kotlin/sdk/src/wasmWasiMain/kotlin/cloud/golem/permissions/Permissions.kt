@file:OptIn(kotlin.wasm.unsafe.UnsafeWasmMemoryApi::class, kotlin.wasm.ExperimentalWasmInterop::class)

package cloud.golem.permissions

import cloud.golem.runtime.AgentId
import cloud.golem.runtime.Either
import cloud.golem.runtime.SchemaValue
import cloud.golem.runtime.Uuid
import cloud.golem.runtime.lowerStringToPtrLen
import cloud.golem.wasm.alloc
import cloud.golem.wasm.liftString
import cloud.golem.wasm.loadByte
import cloud.golem.wasm.loadInt
import cloud.golem.wasm.loadLong
import cloud.golem.wasm.storeInt

// Raw canonical-ABI bindings to golem:permissions@0.1.0 (types, inspect, derive, revoke, wallet, kernel-introspection) and the
// `permission-card` resource drop in golem:core/types@2.0.0. Layouts computed from `wasm-tools component wit --json`
// (pattern-grant 40 B; card-view 32 B; permission-error 12 B = tag + string; the result<_, permission-error> family 16 B,
// result<card-view, ..> 36 B; persistent-card / wallet-card 8 B = tag@0 + handle@4; card-id 16 B; timestamp {s64, u32} 16 B align 8).
private const val TYPES = "golem:permissions/types@0.1.0"
private const val INSPECT = "golem:permissions/inspect@0.1.0"
private const val DERIVE = "golem:permissions/derive@0.1.0"
private const val REVOKE = "golem:permissions/revoke@0.1.0"
private const val WALLET = "golem:permissions/wallet@0.1.0"
private const val KERNEL = "golem:permissions/kernel-introspection@0.1.0"

@kotlin.wasm.WasmImport(TYPES, "id")
private external fun hostCardId(card: Int, retPtr: Int)

@kotlin.wasm.WasmImport(TYPES, "parents")
private external fun hostCardParents(card: Int, retPtr: Int)

@kotlin.wasm.WasmImport(TYPES, "expires-at")
private external fun hostCardExpiresAt(card: Int, retPtr: Int)

@kotlin.wasm.WasmImport(INSPECT, "inspect-card")
private external fun hostInspectCard(card: Int, retPtr: Int)

// derive(parent, lower-positive-to-retain, lower-negative-to-add, upper-positive-to-retain, upper-negative-to-add, lifespan)
@kotlin.wasm.WasmImport(DERIVE, "derive")
private external fun hostDerive(
    parent: Int,
    lpP: Int,
    lpL: Int,
    lnP: Int,
    lnL: Int,
    upP: Int,
    upL: Int,
    unP: Int,
    unL: Int,
    lifeTag: Int,
    lifeP: Int,
    lifeL: Int,
    retPtr: Int,
)

@kotlin.wasm.WasmImport(DERIVE, "derive-from-wallet")
private external fun hostDeriveFromWallet(
    lpP: Int,
    lpL: Int,
    lnP: Int,
    lnL: Int,
    upP: Int,
    upL: Int,
    unP: Int,
    unL: Int,
    lifeTag: Int,
    lifeP: Int,
    lifeL: Int,
    retPtr: Int,
)

@kotlin.wasm.WasmImport(DERIVE, "derive-scope")
private external fun hostDeriveScope(
    parentsP: Int,
    parentsL: Int,
    lpP: Int,
    lpL: Int,
    lnP: Int,
    lnL: Int,
    upP: Int,
    upL: Int,
    unP: Int,
    unL: Int,
    retPtr: Int,
)

@kotlin.wasm.WasmImport(REVOKE, "revoke-card")
private external fun hostRevokeCard(card: Int, retPtr: Int)

@kotlin.wasm.WasmImport(WALLET, "self-wallet")
private external fun hostSelfWallet(retPtr: Int)

@kotlin.wasm.WasmImport(WALLET, "self-version")
private external fun hostSelfVersion(retPtr: Int)

// install-card(card: own<permission-card>, target: holder): the variant flattens to (tag, i64, i64, i32, i32).
@kotlin.wasm.WasmImport(WALLET, "install-card")
private external fun hostInstallCard(card: Int, tag: Int, a: Long, b: Long, c: Int, d: Int, retPtr: Int)

@kotlin.wasm.WasmImport(KERNEL, "list-modules")
private external fun hostListModules(retPtr: Int)

@kotlin.wasm.WasmImport(KERNEL, "validate-grant")
private external fun hostValidateGrant(
    classP: Int,
    classL: Int,
    ownerP: Int,
    ownerL: Int,
    recipientP: Int,
    recipientL: Int,
    verbP: Int,
    verbL: Int,
    resourceP: Int,
    resourceL: Int,
    retPtr: Int,
)

@kotlin.wasm.WasmImport("golem:core/types@2.0.0", "[resource-drop]permission-card")
private external fun hostCardDrop(handle: Int)

// ---------------------------------------------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------------------------------------------

/** An opaque card identity (the card's UUID). Equality is by value; the id is not itself an authority. */
class CardId internal constructor(private val high: Long, private val low: Long) {
    override fun equals(other: Any?): Boolean = other is CardId && other.high == high && other.low == low

    override fun hashCode(): Int = 31 * high.hashCode() + low.hashCode()

    override fun toString(): String = "CardId(<opaque>)"
}

/** A point in time as Unix seconds plus nanoseconds. */
data class Timestamp(val seconds: Long, val nanoseconds: Int)

/** How long a derived card lives, as an ISO-8601 duration of the form `PnDTnHnMnS`; years, months and weeks are not accepted. */
class Lifespan private constructor(val iso8601: String) {
    override fun equals(other: Any?): Boolean = other is Lifespan && other.iso8601 == iso8601

    override fun hashCode(): Int = iso8601.hashCode()

    override fun toString(): String = "Lifespan($iso8601)"

    companion object {
        private val FORM = Regex("""P(?:\d+D)?(?:T(?:\d+H)?(?:\d+M)?(?:\d+(?:\.\d{1,9})?S)?)?""")

        /** A lifespan given as an ISO-8601 duration such as `PT1H` or `P1DT2H`. */
        fun iso8601(value: String): Lifespan {
            require(value.startsWith("P")) { "an ISO-8601 lifespan must start with 'P': $value" }
            require(FORM.matches(value) && value != "P" && !value.endsWith("T")) { "not a duration of the form PnDTnHnMnS: $value" }
            return Lifespan(value)
        }

        /** A lifespan of [duration]: it must be positive and fit in 64-bit nanoseconds. */
        fun of(duration: kotlin.time.Duration): Lifespan {
            require(duration.isPositive()) { "a lifespan must be positive: $duration" }
            require(duration.isFinite() && duration.inWholeNanoseconds < Long.MAX_VALUE) { "the lifespan $duration exceeds the supported range" }
            return Lifespan(durationText(duration.inWholeNanoseconds))
        }

        private fun durationText(totalNanos: Long): String {
            val nanosPerSecond = 1_000_000_000L
            val seconds = totalNanos / nanosPerSecond
            val nanos = (totalNanos % nanosPerSecond).toInt()
            val days = seconds / 86_400
            val hours = seconds % 86_400 / 3_600
            val minutes = seconds % 3_600 / 60
            val wholeSeconds = seconds % 60
            return buildString {
                append('P')
                if (days != 0L) append(days).append('D')
                if (hours != 0L || minutes != 0L || wholeSeconds != 0L || nanos != 0) {
                    append('T')
                    if (hours != 0L) append(hours).append('H')
                    if (minutes != 0L) append(minutes).append('M')
                    if (wholeSeconds != 0L || nanos != 0) {
                        append(wholeSeconds)
                        if (nanos != 0) append('.').append(nanos.toString().padStart(9, '0').trimEnd('0'))
                        append('S')
                    }
                }
            }
        }
    }
}

/** One permission pattern: `class(owner) @ recipient : verb : resource-id`. */
data class PatternGrant(
    val className: String,
    val owner: String,
    val recipient: String,
    val verb: String,
    val resourceId: String,
)

/** The grants a card carries: its lower (granted) and upper (delegable) bounds, positive and negative. */
data class CardView(
    val lowerPositive: List<PatternGrant>,
    val lowerNegative: List<PatternGrant>,
    val upperPositive: List<PatternGrant>,
    val upperNegative: List<PatternGrant>,
)

/** The four grant lists a derived card is requested with (everything not retained / added is dropped). */
data class Bounds(
    val lowerPositive: List<PatternGrant> = emptyList(),
    val lowerNegative: List<PatternGrant> = emptyList(),
    val upperPositive: List<PatternGrant> = emptyList(),
    val upperNegative: List<PatternGrant> = emptyList(),
) {
    companion object {
        val empty: Bounds = Bounds()
    }
}

/** An opaque version token of the agent's wallet. */
class WalletVersionToken internal constructor(val walletIdHash: List<UByte>, val generation: ULong)

/** A kernel resource class and the verbs it accepts. */
data class KernelModule(val className: String, val verbs: List<String>)

/** Who a card is installed on. */
sealed class Holder {
    data class Account(val accountId: Uuid) : Holder()
    data class App(val appId: Uuid) : Holder()
    data class Agent(val agentId: AgentId) : Holder()
}

/** `golem:permissions/types`' `permission-error`. */
sealed class PermissionError {
    abstract val message: String

    /** The WIT case name, e.g. `not-permitted`. */
    val kind: String
        get() = when (this) {
            is NotPermitted -> "not-permitted"
            is LowerBoundTooBroad -> "lower-bound-too-broad"
            is UpperBoundTooBroad -> "upper-bound-too-broad"
            is WalletMultiSourceRequired -> "wallet-multi-source-required"
            is CardRevoked -> "card-revoked"
            is CardExpired -> "card-expired"
            is InstallRecipientMismatch -> "install-recipient-mismatch"
            is UnknownResourceClass -> "unknown-resource-class"
            is InvalidPattern -> "invalid-pattern"
            is InvalidVerb -> "invalid-verb"
            is InvalidOwner -> "invalid-owner"
            is InvalidRecipient -> "invalid-recipient"
        }

    final override fun toString(): String = "$kind: $message"

    data class NotPermitted(override val message: String) : PermissionError()
    data class LowerBoundTooBroad(override val message: String) : PermissionError()
    data class UpperBoundTooBroad(override val message: String) : PermissionError()
    data class WalletMultiSourceRequired(override val message: String) : PermissionError()
    data class CardRevoked(override val message: String) : PermissionError()
    data class CardExpired(override val message: String) : PermissionError()
    data class InstallRecipientMismatch(override val message: String) : PermissionError()
    data class UnknownResourceClass(override val message: String) : PermissionError()
    data class InvalidPattern(override val message: String) : PermissionError()
    data class InvalidVerb(override val message: String) : PermissionError()
    data class InvalidOwner(override val message: String) : PermissionError()
    data class InvalidRecipient(override val message: String) : PermissionError()
}

// ---------------------------------------------------------------------------------------------------------------
// Lifting / lowering
// ---------------------------------------------------------------------------------------------------------------

private const val MOVED = "this permission card has already been moved (sent, installed or revoked) or released"
private const val GRANT_SIZE = 40
private const val ERROR_TAG_OFFSET = 4

internal fun liftPermissionError(base: Int): PermissionError {
    val message = liftString(loadInt(base + 4), loadInt(base + 8))
    return when (val tag = loadByte(base).toInt() and 0xFF) {
        0 -> PermissionError.NotPermitted(message)
        1 -> PermissionError.LowerBoundTooBroad(message)
        2 -> PermissionError.UpperBoundTooBroad(message)
        3 -> PermissionError.WalletMultiSourceRequired(message)
        4 -> PermissionError.CardRevoked(message)
        5 -> PermissionError.CardExpired(message)
        6 -> PermissionError.InstallRecipientMismatch(message)
        7 -> PermissionError.UnknownResourceClass(message)
        8 -> PermissionError.InvalidPattern(message)
        9 -> PermissionError.InvalidVerb(message)
        10 -> PermissionError.InvalidOwner(message)
        11 -> PermissionError.InvalidRecipient(message)
        else -> error("unknown permission-error case $tag")
    }
}

/** `result<T, permission-error>`: tag@0, ok payload and error both at +4. */
private inline fun <T> liftResult(ret: Int, ok: (Int) -> T): Either<PermissionError, T> = if (loadByte(ret).toInt() and 0xFF == 0) {
    Either.Right(ok(ret + ERROR_TAG_OFFSET))
} else {
    Either.Left(liftPermissionError(ret + ERROR_TAG_OFFSET))
}

private fun liftGrant(base: Int) = PatternGrant(
    liftString(loadInt(base), loadInt(base + 4)),
    liftString(loadInt(base + 8), loadInt(base + 12)),
    liftString(loadInt(base + 16), loadInt(base + 20)),
    liftString(loadInt(base + 24), loadInt(base + 28)),
    liftString(loadInt(base + 32), loadInt(base + 36)),
)

private fun liftGrants(listBase: Int): List<PatternGrant> {
    val ptr = loadInt(listBase)
    return List(loadInt(listBase + 4)) { liftGrant(ptr + it * GRANT_SIZE) }
}

private fun liftCardView(base: Int) = CardView(liftGrants(base), liftGrants(base + 8), liftGrants(base + 16), liftGrants(base + 24))

private fun liftCardId(base: Int) = CardId(loadLong(base), loadLong(base + 8))

/** A lowered `list<T>` as `(pointer, length)` words. */
private class ListWords(val ptr: Int, val len: Int)

private fun lowerGrants(grants: List<PatternGrant>): ListWords {
    if (grants.isEmpty()) return ListWords(0, 0)
    val arr = alloc(grants.size * GRANT_SIZE, 4)
    grants.forEachIndexed { i, g ->
        var at = arr + i * GRANT_SIZE
        for (s in listOf(g.className, g.owner, g.recipient, g.verb, g.resourceId)) {
            val (p, l) = lowerStringToPtrLen(s)
            storeInt(at, p)
            storeInt(at + 4, l)
            at += 8
        }
    }
    return ListWords(arr, grants.size)
}

/** `option<duration>` as `(tag, ptr, len)` words: `duration` is a record of one string. */
private fun lowerLifespan(lifespan: Lifespan?): Triple<Int, Int, Int> {
    if (lifespan == null) return Triple(0, 0, 0)
    val (p, l) = lowerStringToPtrLen(lifespan.iso8601)
    return Triple(1, p, l)
}

// ---------------------------------------------------------------------------------------------------------------
// Cards
// ---------------------------------------------------------------------------------------------------------------

/** The owner of one host handle: it hands the handle out once, after which every use fails loudly. */
internal class HandleSlot(private var handle: Int) {
    val isPresent: Boolean get() = handle != 0

    fun borrow(): Int {
        check(handle != 0) { MOVED }
        return handle
    }

    fun take(): Int {
        val taken = borrow()
        handle = 0
        return taken
    }

    /** `Type(<present>)` or `Type(<moved>)`; the handle number is never shown. */
    fun describe(type: String): String = "$type(${if (isPresent) "<present>" else "<moved>"})"
}

/**
 * A handle to a permission card held by the runtime. Cards are unforgeable: the only operations are those of the
 * `golem:permissions` interfaces. A card's own handle **moves** when the card is sent to another agent, installed or
 * revoked; afterwards (and after [release]) every operation throws [IllegalStateException]. [release] drops the handle
 * when you are done with a card you keep (cards are not reclaimed by Kotlin/Wasm GC).
 */
sealed class Card(handle: Int) {
    internal val slot = HandleSlot(handle)

    /** True until the card's handle moved or was released. */
    val isPresent: Boolean get() = slot.isPresent

    internal fun borrowHandle(): Int = slot.borrow()

    /** Takes the handle out of this card (it becomes absent): used when the handle is consumed by the host. */
    internal fun takeHandle(): Int = slot.take()

    /** The card's identity. */
    val id: CardId
        get() {
            val ret = alloc(16, 8)
            hostCardId(borrowHandle(), ret)
            return liftCardId(ret)
        }

    /** The identities of the cards this one was derived from. */
    val parents: List<CardId>
        get() {
            val ret = alloc(8, 4)
            hostCardParents(borrowHandle(), ret)
            val ptr = loadInt(ret)
            return List(loadInt(ret + 4)) { liftCardId(ptr + it * 16) }
        }

    /** When the card expires, if it does. */
    val expiresAt: Timestamp?
        get() {
            val ret = alloc(24, 8) // option<timestamp>: tag@0, seconds@8, nanoseconds@16
            hostCardExpiresAt(borrowHandle(), ret)
            return if (loadByte(ret).toInt() and 0xFF == 0) null else Timestamp(loadLong(ret + 8), loadInt(ret + 16))
        }

    /** The grants the card carries, or why the caller may not look at them. */
    fun inspect(): Either<PermissionError, CardView> {
        val ret = alloc(36, 4)
        hostInspectCard(borrowHandle(), ret)
        return liftResult(ret) { liftCardView(it) }
    }

    /**
     * Moves this card into a schema value (an agent method argument or result): the receiver owns the handle, and this card
     * becomes absent. Used by generated code; sending a card is the way to pass it to another agent.
     */
    fun intoSchemaValue(): SchemaValue.PermissionCardVal = SchemaValue.PermissionCardVal(takeHandle())

    /** Drops the handle. A card that already moved or was released is left alone. */
    fun release() {
        if (isPresent) hostCardDrop(takeHandle())
    }

    internal fun deriveChild(bounds: Bounds, lifespan: Lifespan?): Either<PermissionError, PersistentCard> {
        val parent = borrowHandle()
        val lp = lowerGrants(bounds.lowerPositive)
        val ln = lowerGrants(bounds.lowerNegative)
        val up = lowerGrants(bounds.upperPositive)
        val un = lowerGrants(bounds.upperNegative)
        val (tag, p, l) = lowerLifespan(lifespan)
        val ret = alloc(16, 4)
        hostDerive(parent, lp.ptr, lp.len, ln.ptr, ln.len, up.ptr, up.len, un.ptr, un.len, tag, p, l, ret)
        return liftResult(ret) { PersistentCard.fromHost(it) }
    }

    internal fun revokeCard(): Either<PermissionError, Long> {
        val ret = alloc(16, 4)
        hostRevokeCard(takeHandle(), ret)
        return liftResult(ret) { (loadInt(it).toLong() and 0xFFFFFFFFL) }
    }

    internal fun installCard(target: Holder): Either<PermissionError, Unit> {
        val handle = takeHandle()
        val ret = alloc(16, 4)
        when (target) {
            is Holder.Account -> hostInstallCard(handle, 0, target.accountId.highBits, target.accountId.lowBits, 0, 0, ret)
            is Holder.App -> hostInstallCard(handle, 1, target.appId.highBits, target.appId.lowBits, 0, 0, ret)
            is Holder.Agent -> {
                val (p, l) = lowerStringToPtrLen(target.agentId.agentId)
                val uuid = target.agentId.componentId.uuid
                hostInstallCard(handle, 2, uuid.highBits, uuid.lowBits, p, l, ret)
            }
        }
        return liftResult(ret) { }
    }
}

/** A concrete permission card. */
class PermissionCard internal constructor(handle: Int) : Card(handle) {
    /** Derives a child card limited to [bounds], optionally expiring after [lifespan]. */
    fun derive(bounds: Bounds, lifespan: Lifespan? = null): Either<PermissionError, PersistentCard> = deriveChild(bounds, lifespan)

    /** Revokes the card and everything derived from it; returns how many cards were revoked. Moves the handle. */
    fun revoke(): Either<PermissionError, Long> = revokeCard()

    /** Installs the card on [target]. Moves the handle. */
    fun install(target: Holder): Either<PermissionError, Unit> = installCard(target)

    override fun toString(): String = slot.describe("PermissionCard")

    companion object {
        /** Wraps a handle received through the schema layer (an agent method parameter). */
        fun fromHandle(handle: Int): PermissionCard = PermissionCard(handle)
    }
}

/** A polymorphic (template) permission card: its slot variables are filled in when it is derived. */
class PolymorphicPermissionCard internal constructor(handle: Int) : Card(handle) {
    fun derive(bounds: Bounds, lifespan: Lifespan? = null): Either<PermissionError, PersistentCard> = deriveChild(bounds, lifespan)

    fun revoke(): Either<PermissionError, Long> = revokeCard()

    fun install(target: Holder): Either<PermissionError, Unit> = installCard(target)

    override fun toString(): String = slot.describe("PolymorphicPermissionCard")

    companion object {
        /** Wraps a handle received through the schema layer (an agent method parameter). */
        fun fromHandle(handle: Int): PolymorphicPermissionCard = PolymorphicPermissionCard(handle)
    }
}

/** A short-lived card lent to a callee for the duration of an RPC call (see `withScopeCard`). It cannot be derived from. */
class ScopeCard internal constructor(handle: Int) : Card(handle) {
    override fun toString(): String = "ScopeCard(..)"
}

/** A card the host persisted: a concrete card or a template. Forwards to the wrapped card. */
sealed class PersistentCard {
    abstract val card: Card

    class Concrete internal constructor(override val card: PermissionCard) : PersistentCard()

    class Template internal constructor(override val card: PolymorphicPermissionCard) : PersistentCard()

    val isPresent: Boolean get() = card.isPresent
    val id: CardId get() = card.id
    val parents: List<CardId> get() = card.parents
    val expiresAt: Timestamp? get() = card.expiresAt

    fun inspect(): Either<PermissionError, CardView> = card.inspect()

    fun derive(bounds: Bounds, lifespan: Lifespan? = null): Either<PermissionError, PersistentCard> = card.deriveChild(bounds, lifespan)

    fun revoke(): Either<PermissionError, Long> = card.revokeCard()

    fun install(target: Holder): Either<PermissionError, Unit> = card.installCard(target)

    fun release() = card.release()

    internal companion object {
        /** `persistent-card`: tag@0 (0 = concrete, 1 = template), handle@4. */
        fun fromHost(base: Int): PersistentCard {
            val handle = loadInt(base + 4)
            return when (val tag = loadByte(base).toInt() and 0xFF) {
                0 -> Concrete(PermissionCard(handle))
                1 -> Template(PolymorphicPermissionCard(handle))
                else -> error("unknown persistent-card case $tag")
            }
        }
    }
}

/** An entry of the agent's wallet. */
sealed class WalletCard {
    abstract val card: Card

    class Concrete internal constructor(override val card: PermissionCard) : WalletCard()

    class Template internal constructor(override val card: PolymorphicPermissionCard) : WalletCard()

    class Scope internal constructor(override val card: ScopeCard) : WalletCard()
}

/** The agent's view of the permission system: its wallet, card derivation and the kernel's resource classes. */
object Permissions {
    /** Every card the agent currently holds (each is a fresh handle; [Card.release] the ones you do not keep). */
    fun wallet(): List<WalletCard> {
        val ret = alloc(8, 4)
        hostSelfWallet(ret)
        val ptr = loadInt(ret)
        return List(loadInt(ret + 4)) { i ->
            val entry = ptr + i * 8 // wallet-card: tag@0 (0 concrete, 1 template, 2 scope), handle@4
            val handle = loadInt(entry + 4)
            when (val tag = loadByte(entry).toInt() and 0xFF) {
                0 -> WalletCard.Concrete(PermissionCard(handle))
                1 -> WalletCard.Template(PolymorphicPermissionCard(handle))
                2 -> WalletCard.Scope(ScopeCard(handle))
                else -> error("unknown wallet-card case $tag")
            }
        }
    }

    /** The wallet's current version token. */
    fun selfVersion(): WalletVersionToken {
        val ret = alloc(16, 8) // { wallet-id-hash: list<u8>@0, generation: u64@8 }
        hostSelfVersion(ret)
        val ptr = loadInt(ret)
        val hash = List(loadInt(ret + 4)) { loadByte(ptr + it).toUByte() }
        return WalletVersionToken(hash, loadLong(ret + 8).toULong())
    }

    /** Derives a card from the wallet's cards (all of the wallet must be able to grant [bounds]). */
    fun deriveFromWallet(bounds: Bounds, lifespan: Lifespan? = null): Either<PermissionError, PersistentCard> {
        val lp = lowerGrants(bounds.lowerPositive)
        val ln = lowerGrants(bounds.lowerNegative)
        val up = lowerGrants(bounds.upperPositive)
        val un = lowerGrants(bounds.upperNegative)
        val (tag, p, l) = lowerLifespan(lifespan)
        val ret = alloc(16, 4)
        hostDeriveFromWallet(lp.ptr, lp.len, ln.ptr, ln.len, up.ptr, up.len, un.ptr, un.len, tag, p, l, ret)
        return liftResult(ret) { PersistentCard.fromHost(it) }
    }

    /**
     * Derives a scope card from [parents] (borrowed, not consumed). A scope card can be lent to a callee for one RPC call
     * and expires with its earliest parent.
     */
    fun deriveScope(parents: List<Card>, bounds: Bounds = Bounds.empty): Either<PermissionError, ScopeCard> {
        val handles = if (parents.isEmpty()) 0 else alloc(parents.size * 4, 4)
        parents.forEachIndexed { i, card -> storeInt(handles + i * 4, card.borrowHandle()) }
        val lp = lowerGrants(bounds.lowerPositive)
        val ln = lowerGrants(bounds.lowerNegative)
        val up = lowerGrants(bounds.upperPositive)
        val un = lowerGrants(bounds.upperNegative)
        val ret = alloc(16, 4)
        hostDeriveScope(handles, parents.size, lp.ptr, lp.len, ln.ptr, ln.len, up.ptr, up.len, un.ptr, un.len, ret)
        return liftResult(ret) { ScopeCard(loadInt(it)) }
    }

    /** The resource classes the kernel knows, with the verbs each accepts. */
    fun kernelModules(): List<KernelModule> {
        val ret = alloc(8, 4)
        hostListModules(ret)
        val ptr = loadInt(ret)
        return List(loadInt(ret + 4)) { i ->
            val module = ptr + i * 16 // kernel-module: class-name@0, verbs: list<string>@8
            val verbs = loadInt(module + 8)
            KernelModule(
                liftString(loadInt(module), loadInt(module + 4)),
                List(loadInt(module + 12)) { liftString(loadInt(verbs + it * 8), loadInt(verbs + it * 8 + 4)) },
            )
        }
    }

    /** Checks that [grant] is well-formed for the kernel (known class, verb and patterns). */
    fun validateGrant(grant: PatternGrant): Either<PermissionError, Unit> {
        val parts = listOf(grant.className, grant.owner, grant.recipient, grant.verb, grant.resourceId).map { lowerStringToPtrLen(it) }
        val ret = alloc(16, 4)
        hostValidateGrant(
            parts[0].first, parts[0].second, parts[1].first, parts[1].second, parts[2].first, parts[2].second,
            parts[3].first, parts[3].second, parts[4].first, parts[4].second, ret,
        )
        return liftResult(ret) { }
    }
}
