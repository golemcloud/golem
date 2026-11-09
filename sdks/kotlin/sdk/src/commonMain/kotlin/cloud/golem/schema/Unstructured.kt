package cloud.golem.schema

/**
 * Free-form text that is not described by a structured schema: either the text inline (with an optional BCP-47 language code) or a URL
 * that refers to it. Its schema is `variant { inline: text, url: url }` with the `unstructured-text` role. Restrict the languages with
 * `@Languages("en", "de") UnstructuredText` on the type; without it every language code, or none, is accepted.
 */
sealed class UnstructuredText {
    data class Inline(val text: String, val languageCode: String? = null) : UnstructuredText()

    data class Url(val url: String) : UnstructuredText()

    companion object {
        fun fromInline(text: String, languageCode: String? = null): UnstructuredText = Inline(text, languageCode)

        fun fromUrl(url: String): UnstructuredText = Url(url)
    }
}

/**
 * Free-form binary content (documents, images, generated files): inline bytes with an optional MIME type, or a URL that refers to the
 * content. Its schema is `variant { inline: binary, url: url }` with the `unstructured-binary` role. Restrict the MIME types with
 * `@MimeTypes("image/png") UnstructuredBinary` on the type; without it every MIME type, or none, is accepted.
 *
 * An HTTP endpoint that returns an inline value renders the raw bytes as the response body.
 */
sealed class UnstructuredBinary {
    class Inline(val bytes: ByteArray, val mimeType: String? = null) : UnstructuredBinary() {
        override fun equals(other: Any?): Boolean = other is Inline && other.mimeType == mimeType && other.bytes.contentEquals(bytes)

        override fun hashCode(): Int = 31 * bytes.contentHashCode() + (mimeType?.hashCode() ?: 0)

        override fun toString(): String = "Inline(${bytes.size} bytes, mimeType=$mimeType)"
    }

    data class Url(val url: String) : UnstructuredBinary()

    companion object {
        fun fromInline(bytes: ByteArray, mimeType: String? = null): UnstructuredBinary = Inline(bytes, mimeType)

        fun fromUrl(url: String): UnstructuredBinary = Url(url)
    }
}
