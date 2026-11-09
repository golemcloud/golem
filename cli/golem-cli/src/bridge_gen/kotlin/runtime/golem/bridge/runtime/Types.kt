/*
 * Copyright 2024-2026 Golem Cloud
 *
 * Licensed under the Golem Source License v1.1 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://license.golem.cloud/LICENSE
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

package golem.bridge.runtime

import java.time.Instant

/** A UUID as its two 64-bit halves, which is how it travels in a schema value. */
data class Uuid(val highBits: Long, val lowBits: Long) {
    /** The canonical lower-case `8-4-4-4-12` form. */
    fun toStandardString(): String {
        val hex = hex16(highBits) + hex16(lowBits)
        return "${hex.substring(0, 8)}-${hex.substring(8, 12)}-${hex.substring(12, 16)}-${hex.substring(16, 20)}-${hex.substring(20)}"
    }

    override fun toString(): String = toStandardString()

    companion object {
        fun random(): Uuid {
            val uuid = java.util.UUID.randomUUID()
            return Uuid(uuid.mostSignificantBits, uuid.leastSignificantBits)
        }

        /** Parses the canonical `8-4-4-4-12` form (case-insensitive hexadecimal digits). */
        fun fromStandardString(text: String): Uuid {
            if (!CANONICAL.matches(text)) throw BridgeException("Not a canonical UUID: '$text'")
            val hex = text.replace("-", "")
            return Uuid(java.lang.Long.parseUnsignedLong(hex.substring(0, 16), 16), java.lang.Long.parseUnsignedLong(hex.substring(16), 16))
        }

        private val CANONICAL = Regex("[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}")

        private fun hex16(value: Long): String = java.lang.Long.toHexString(value).padStart(16, '0')
    }
}

/** A point in time: whole seconds since the Unix epoch and the nanoseconds within the second. */
data class Datetime(val seconds: Long, val nanoseconds: Int) {
    /** The RFC 3339 UTC form the server reads. */
    fun toIsoString(): String {
        val text = toInstant().toString()
        // `Instant.toString` uses ISO_INSTANT, which prints a year outside 0000..9999
        // with a sign and extra digits (`+10000-01-01T00:00:00Z`). The server parses
        // this as RFC 3339, which admits neither, so fail here naming the value
        // rather than let it come back as a server-side parse error.
        if (text.startsWith("+") || text.startsWith("-")) {
            throw BridgeException("Datetime $seconds s + $nanoseconds ns is outside the years RFC 3339 can express (0000..9999)")
        }
        return text
    }

    fun toInstant(): Instant = try {
        Instant.ofEpochSecond(seconds, nanoseconds.toLong())
    } catch (e: java.time.DateTimeException) {
        throw BridgeException("Datetime $seconds s + $nanoseconds ns is outside the range of an instant", e)
    } catch (e: ArithmeticException) {
        throw BridgeException("Datetime $seconds s + $nanoseconds ns is outside the range of an instant", e)
    }

    companion object {
        fun fromInstant(instant: Instant): Datetime = Datetime(instant.epochSecond, instant.nano)

        fun parse(text: String): Datetime = try {
            fromInstant(Instant.parse(text))
        } catch (e: java.time.format.DateTimeParseException) {
            throw BridgeException("Invalid datetime '$text'", e)
        }
    }
}

/**
 * A number in a unit: the value is `mantissa * 10^-scale` in [unit]. The schema names the base unit and the
 * other units a value may use, and an empty [unit] means the base unit.
 */
data class Quantity(val mantissa: Long, val scale: Int, val unit: String = "")

/** The outcome of a `result<ok, err>` value: [Right] is the ok arm and [Left] the error arm. */
sealed class Either<out L, out R> {
    data class Left<out L>(val value: L) : Either<L, Nothing>()

    data class Right<out R>(val value: R) : Either<Nothing, R>()
}

/** Free-form text: inline, with an optional language code, or a URL that refers to it. */
sealed class UnstructuredText {
    data class Inline(val text: String, val languageCode: String? = null) : UnstructuredText()

    data class Url(val url: String) : UnstructuredText()
}

/** Free-form binary content: inline bytes with an optional MIME type, or a URL that refers to it. */
sealed class UnstructuredBinary {
    class Inline(val bytes: ByteArray, val mimeType: String? = null) : UnstructuredBinary() {
        override fun equals(other: Any?): Boolean = other is Inline && other.mimeType == mimeType && other.bytes.contentEquals(bytes)

        override fun hashCode(): Int = 31 * bytes.contentHashCode() + (mimeType?.hashCode() ?: 0)

        override fun toString(): String = "Inline(${bytes.size} bytes, mimeType=$mimeType)"
    }

    data class Url(val url: String) : UnstructuredBinary()
}

/** The identity of a resolved agent, as the server reports it. */
data class AgentId(val componentId: String, val agentId: String)

/** The target Golem server of the generated clients. */
sealed class GolemServer {
    /** Base URL of the worker service, without a trailing slash. */
    abstract val url: String

    /** Bearer token for the `Authorization` header. */
    abstract val token: String

    /** The local single-executable server on `http://localhost:9881`. */
    object Local : GolemServer() {
        override val url: String = "http://localhost:9881"
        override val token: String = "5c832d93-ff85-4a8f-9803-513950fdfdb1"
    }

    /** Golem Cloud (release region). */
    data class Cloud(override val token: String) : GolemServer() {
        override val url: String = "https://release.api.golem.cloud"
    }

    /** A custom worker-service deployment. */
    data class Custom(override val url: String, override val token: String) : GolemServer()
}

/** The connection settings shared by the generated clients of this project. */
data class Configuration(val server: GolemServer, val appName: String, val envName: String) {
    companion object {
        @Volatile
        private var current: Configuration? = null

        /** Points every generated client at [server]. May be called again to retarget the clients. */
        fun configure(server: GolemServer, appName: String, envName: String) {
            current = Configuration(server, appName, envName)
        }

        /** The active configuration; throws until [configure] has been called. */
        fun get(): Configuration = current
            ?: throw BridgeException("Golem bridge configuration has not been set. Call Configuration.configure(...) first.")

        fun isConfigured(): Boolean = current != null
    }
}
