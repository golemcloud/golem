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

package golem.bridge.runtime.json

import golem.bridge.runtime.BridgeException

/**
 * A JSON value. Numbers keep their exact decimal literal, so a full-width 64-bit integer and a negative zero survive a round trip.
 * Object members keep their insertion order, so encoded request bodies are deterministic.
 */
sealed class Json {
    object Null : Json()

    data class Bool(val value: Boolean) : Json()

    data class Num(val literal: String) : Json()

    data class Str(val value: String) : Json()

    data class Arr(val items: List<Json>) : Json()

    data class Obj(val fields: List<Pair<String, Json>>) : Json()

    /** Renders this value as compact JSON. */
    fun render(): String = StringBuilder().also { write(it) }.toString()

    override fun toString(): String = render()

    internal fun write(out: StringBuilder) {
        when (this) {
            Null -> out.append("null")
            is Bool -> out.append(if (value) "true" else "false")
            is Num -> out.append(literal)
            is Str -> writeString(value, out)
            is Arr -> {
                out.append('[')
                items.forEachIndexed { i, item ->
                    if (i != 0) out.append(',')
                    item.write(out)
                }
                out.append(']')
            }
            is Obj -> {
                out.append('{')
                fields.forEachIndexed { i, (name, value) ->
                    if (i != 0) out.append(',')
                    writeString(name, out)
                    out.append(':')
                    value.write(out)
                }
                out.append('}')
            }
        }
    }

    companion object {
        fun string(value: String): Json = Str(value)

        fun bool(value: Boolean): Json = Bool(value)

        fun int(value: Int): Json = Num(value.toString())

        fun long(value: Long): Json = Num(value.toString())

        /** A number from its decimal literal, which the caller guarantees is valid JSON. */
        fun number(literal: String): Json = Num(literal)

        fun arr(items: List<Json>): Json = Arr(items)

        fun obj(vararg fields: Pair<String, Json>): Json = Obj(fields.toList())

        fun obj(fields: List<Pair<String, Json>>): Json = Obj(fields)

        fun asObject(json: Json): List<Pair<String, Json>> = (json as? Obj)?.fields ?: fail("Expected a JSON object, got ${typeName(json)}")

        fun asArray(json: Json): List<Json> = (json as? Arr)?.items ?: fail("Expected a JSON array, got ${typeName(json)}")

        fun asString(json: Json): String = (json as? Str)?.value ?: fail("Expected a JSON string, got ${typeName(json)}")

        fun asBoolean(json: Json): Boolean = (json as? Bool)?.value ?: fail("Expected a JSON boolean, got ${typeName(json)}")

        fun asNumberLiteral(json: Json): String = (json as? Num)?.literal ?: fail("Expected a JSON number, got ${typeName(json)}")

        /** The member `name` of an object; absent and an explicit `null` are the same. */
        fun field(json: Json, name: String): Json? = (json as? Obj)?.fields?.firstOrNull { it.first == name }?.second?.takeIf { it != Null }

        fun requireField(json: Json, name: String): Json = field(json, name) ?: fail("Missing required field '$name'")

        /** Parses [input], which must hold exactly one JSON value. */
        fun parse(input: String): Json = Parser(input).parseDocument()

        private fun typeName(json: Json): String = when (json) {
            Null -> "null"
            is Bool -> "boolean"
            is Num -> "number"
            is Str -> "string"
            is Arr -> "array"
            is Obj -> "object"
        }

        private fun fail(message: String): Nothing = throw BridgeException(message)

        private fun writeString(value: String, out: StringBuilder) {
            out.append('"')
            for ((i, c) in value.withIndex()) {
                if (Character.isHighSurrogate(c) && !(i + 1 < value.length && Character.isLowSurrogate(value[i + 1])) ||
                    Character.isLowSurrogate(c) && !(i > 0 && Character.isHighSurrogate(value[i - 1]))
                ) {
                    throw BridgeException("A string with a lone surrogate (U+${c.code.toString(16).uppercase()}) cannot be written as JSON")
                }
                when {
                    c == '"' -> out.append("\\\"")
                    c == '\\' -> out.append("\\\\")
                    c == '\n' -> out.append("\\n")
                    c == '\r' -> out.append("\\r")
                    c == '\t' -> out.append("\\t")
                    c == '\b' -> out.append("\\b")
                    c == '\u000C' -> out.append("\\f")
                    c < ' ' -> out.append("\\u").append(c.code.toString(16).padStart(4, '0'))
                    else -> out.append(c)
                }
            }
            out.append('"')
        }
    }

    private class Parser(private val text: String) {
        private var pos = 0

        fun parseDocument(): Json {
            skipWhitespace()
            val value = parseValue(0)
            skipWhitespace()
            if (pos != text.length) fail("Unexpected trailing content at offset $pos")
            return value
        }

        private fun parseValue(depth: Int): Json {
            if (depth > MAX_DEPTH) fail("JSON nesting exceeds $MAX_DEPTH levels")
            if (pos >= text.length) fail("Unexpected end of JSON input")
            return when (val c = text[pos]) {
                '{' -> parseObject(depth)
                '[' -> parseArray(depth)
                '"' -> Str(parseString())
                't' -> literal("true", Bool(true))
                'f' -> literal("false", Bool(false))
                'n' -> literal("null", Null)
                else -> if (c == '-' || c in '0'..'9') parseNumber() else fail("Unexpected character '$c' at offset $pos")
            }
        }

        private fun literal(word: String, value: Json): Json {
            if (!text.startsWith(word, pos)) fail("Invalid literal at offset $pos")
            pos += word.length
            return value
        }

        private fun parseObject(depth: Int): Json {
            pos++
            val fields = ArrayList<Pair<String, Json>>()
            skipWhitespace()
            if (peek() == '}') {
                pos++
                return Obj(fields)
            }
            while (true) {
                skipWhitespace()
                if (peek() != '"') fail("Expected an object member name at offset $pos")
                val name = parseString()
                skipWhitespace()
                if (peek() != ':') fail("Expected ':' at offset $pos")
                pos++
                skipWhitespace()
                fields.add(name to parseValue(depth + 1))
                skipWhitespace()
                when (peek()) {
                    ',' -> pos++
                    '}' -> {
                        pos++
                        return Obj(fields)
                    }
                    else -> fail("Expected ',' or '}' at offset $pos")
                }
            }
        }

        private fun parseArray(depth: Int): Json {
            pos++
            val items = ArrayList<Json>()
            skipWhitespace()
            if (peek() == ']') {
                pos++
                return Arr(items)
            }
            while (true) {
                skipWhitespace()
                items.add(parseValue(depth + 1))
                skipWhitespace()
                when (peek()) {
                    ',' -> pos++
                    ']' -> {
                        pos++
                        return Arr(items)
                    }
                    else -> fail("Expected ',' or ']' at offset $pos")
                }
            }
        }

        private fun parseString(): String {
            pos++
            val out = StringBuilder()
            while (true) {
                if (pos >= text.length) fail("Unterminated string")
                val c = text[pos++]
                when {
                    c == '"' -> return out.toString()
                    c == '\\' -> {
                        if (pos >= text.length) fail("Unterminated escape")
                        when (val e = text[pos++]) {
                            '"', '\\', '/' -> out.append(e)
                            'b' -> out.append('\b')
                            'f' -> out.append('\u000C')
                            'n' -> out.append('\n')
                            'r' -> out.append('\r')
                            't' -> out.append('\t')
                            'u' -> {
                                if (pos + 4 > text.length) fail("Truncated \\u escape")
                                val hex = text.substring(pos, pos + 4)
                                if (!hex.all { it in '0'..'9' || it in 'a'..'f' || it in 'A'..'F' }) fail("Invalid \\u escape '$hex'")
                                val unit = hex.toInt(16).toChar()
                                pos += 4
                                // A surrogate is only valid as a pair. Accepting a lone one would
                                // build a string the printer then refuses to write back out.
                                if (Character.isHighSurrogate(unit)) {
                                    if (pos + 6 > text.length || text[pos] != '\\' || text[pos + 1] != 'u') {
                                        fail("A high surrogate escape must be followed by a low surrogate escape")
                                    }
                                    val lowHex = text.substring(pos + 2, pos + 6)
                                    if (!lowHex.all { it in '0'..'9' || it in 'a'..'f' || it in 'A'..'F' }) fail("Invalid \\u escape '$lowHex'")
                                    val low = lowHex.toInt(16).toChar()
                                    if (!Character.isLowSurrogate(low)) fail("A high surrogate escape must be followed by a low surrogate escape")
                                    out.append(unit)
                                    out.append(low)
                                    pos += 6
                                } else if (Character.isLowSurrogate(unit)) {
                                    fail("A low surrogate escape must follow a high surrogate escape")
                                } else {
                                    out.append(unit)
                                }
                            }
                            else -> fail("Invalid escape '\\$e'")
                        }
                    }
                    c < ' ' -> fail("Unescaped control character in string")
                    else -> out.append(c)
                }
            }
        }

        private fun parseNumber(): Json {
            val start = pos
            if (peek() == '-') pos++
            when (peek()) {
                '0' -> pos++
                in '1'..'9' -> while (peek() in '0'..'9') pos++
                else -> fail("Invalid number at offset $start")
            }
            if (peek() == '.') {
                pos++
                if (peek() !in '0'..'9') fail("Invalid number at offset $start")
                while (peek() in '0'..'9') pos++
            }
            if (peek() == 'e' || peek() == 'E') {
                pos++
                if (peek() == '+' || peek() == '-') pos++
                if (peek() !in '0'..'9') fail("Invalid number at offset $start")
                while (peek() in '0'..'9') pos++
            }
            return Num(text.substring(start, pos))
        }

        private fun peek(): Char = if (pos < text.length) text[pos] else '\u0000'

        private fun skipWhitespace() {
            while (pos < text.length && (text[pos] == ' ' || text[pos] == '\t' || text[pos] == '\n' || text[pos] == '\r')) pos++
        }
    }
}

private const val MAX_DEPTH = 512
