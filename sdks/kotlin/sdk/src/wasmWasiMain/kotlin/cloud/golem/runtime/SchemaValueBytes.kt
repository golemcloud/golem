package cloud.golem.runtime

/**
 * Self-contained recursive codec: [SchemaValue] <-> ByteArray. Needed because the linear-memory
 * schema-value-tree is position-dependent (internal pointers) and can't be detached as portable
 * bytes. Scope: exactly the cases KSP's ConverterCodegen emits for a mapped agent-state type
 * (primitives, record, list, tuple, option, result, variant, enum, map, datetime, unit). Cases
 * outside that set (flags/text/binary/path/url/duration/quantity/union/secret/quota) are never
 * produced for user state and are rejected. Node := [tag: u8][payload]; ints are big-endian.
 */
object SchemaValueBytes {
    private const val BOOL = 1
    private const val S8 = 2
    private const val S16 = 3
    private const val S32 = 4
    private const val S64 = 5
    private const val U8 = 6
    private const val U16 = 7
    private const val U32 = 8
    private const val U64 = 9
    private const val F32 = 10
    private const val F64 = 11
    private const val CHR = 12
    private const val STR = 13
    private const val RECORD = 14
    private const val LIST = 15
    private const val TUPLE = 16
    private const val OPTION = 17
    private const val RESULT = 18
    private const val VARIANT = 19
    private const val ENUM = 20
    private const val MAP = 21
    private const val DATETIME = 22
    private const val UNIT = 23

    fun encode(v: SchemaValue): ByteArray {
        val w = Writer()
        put(w, v)
        return w.toByteArray()
    }
    fun decode(bytes: ByteArray): SchemaValue = get(Reader(bytes))

    private fun put(w: Writer, v: SchemaValue) {
        when (v) {
            is SchemaValue.Bool -> {
                w.u8(BOOL)
                w.u8(if (v.v) 1 else 0)
            }
            is SchemaValue.S8 -> {
                w.u8(S8)
                w.i32(v.v.toInt())
            }
            is SchemaValue.S16 -> {
                w.u8(S16)
                w.i32(v.v.toInt())
            }
            is SchemaValue.S32 -> {
                w.u8(S32)
                w.i32(v.v)
            }
            is SchemaValue.S64 -> {
                w.u8(S64)
                w.i64(v.v)
            }
            is SchemaValue.U8 -> {
                w.u8(U8)
                w.i32(v.v.toInt())
            }
            is SchemaValue.U16 -> {
                w.u8(U16)
                w.i32(v.v.toInt())
            }
            is SchemaValue.U32 -> {
                w.u8(U32)
                w.i32(v.v.toInt())
            }
            is SchemaValue.U64 -> {
                w.u8(U64)
                w.i64(v.v.toLong())
            }
            is SchemaValue.F32 -> {
                w.u8(F32)
                w.i32(v.v.toBits())
            }
            is SchemaValue.F64 -> {
                w.u8(F64)
                w.i64(v.v.toBits())
            }
            is SchemaValue.Chr -> {
                w.u8(CHR)
                w.i32(v.v.code)
            }
            is SchemaValue.Str -> {
                w.u8(STR)
                w.str(v.v)
            }
            is SchemaValue.Record -> {
                w.u8(RECORD)
                w.i32(v.fields.size)
                v.fields.forEach { put(w, it) }
            }
            is SchemaValue.ListVal -> {
                w.u8(LIST)
                w.i32(v.items.size)
                v.items.forEach { put(w, it) }
            }
            is SchemaValue.TupleVal -> {
                w.u8(TUPLE)
                w.i32(v.items.size)
                v.items.forEach { put(w, it) }
            }
            is SchemaValue.OptionVal -> {
                w.u8(OPTION)
                val i = v.inner
                if (i == null) {
                    w.u8(0)
                } else {
                    w.u8(1)
                    put(w, i)
                }
            }
            is SchemaValue.ResultVal -> {
                w.u8(RESULT)
                w.u8(if (v.ok) 1 else 0)
                val i = v.inner
                if (i == null) {
                    w.u8(0)
                } else {
                    w.u8(1)
                    put(w, i)
                }
            }
            is SchemaValue.VariantVal -> {
                w.u8(VARIANT)
                w.i32(v.caseIndex)
                val p = v.payload
                if (p == null) {
                    w.u8(0)
                } else {
                    w.u8(1)
                    put(w, p)
                }
            }
            is SchemaValue.EnumVal -> {
                w.u8(ENUM)
                w.i32(v.caseIndex)
            }
            is SchemaValue.MapVal -> {
                w.u8(MAP)
                w.i32(v.entries.size)
                v.entries.forEach { (k, vv) ->
                    put(w, k)
                    put(w, vv)
                }
            }
            is SchemaValue.DatetimeVal -> {
                w.u8(DATETIME)
                w.i64(v.seconds)
                w.i32(v.nanoseconds)
            }
            SchemaValue.Unit_ -> w.u8(UNIT)
            else -> error("SchemaValueBytes: unsupported case ${v::class.simpleName} (not produced for mapped agent state)")
        }
    }

    private fun get(r: Reader): SchemaValue = when (val tag = r.u8()) {
        BOOL -> SchemaValue.Bool(r.u8() != 0)
        S8 -> SchemaValue.S8(r.i32().toByte())
        S16 -> SchemaValue.S16(r.i32().toShort())
        S32 -> SchemaValue.S32(r.i32())
        S64 -> SchemaValue.S64(r.i64())
        U8 -> SchemaValue.U8(r.i32().toUByte())
        U16 -> SchemaValue.U16(r.i32().toUShort())
        U32 -> SchemaValue.U32(r.i32().toUInt())
        U64 -> SchemaValue.U64(r.i64().toULong())
        F32 -> SchemaValue.F32(Float.fromBits(r.i32()))
        F64 -> SchemaValue.F64(Double.fromBits(r.i64()))
        CHR -> SchemaValue.Chr(r.i32().toChar())
        STR -> SchemaValue.Str(r.str())
        RECORD -> {
            val n = r.i32()
            SchemaValue.Record((0 until n).map { get(r) })
        }
        LIST -> {
            val n = r.i32()
            SchemaValue.ListVal((0 until n).map { get(r) })
        }
        TUPLE -> {
            val n = r.i32()
            SchemaValue.TupleVal((0 until n).map { get(r) })
        }
        OPTION -> SchemaValue.OptionVal(if (r.u8() == 0) null else get(r))
        RESULT -> {
            val ok = r.u8() != 0
            val inner = if (r.u8() == 0) null else get(r)
            SchemaValue.ResultVal(ok, inner)
        }
        VARIANT -> {
            val ci = r.i32()
            val p = if (r.u8() == 0) null else get(r)
            SchemaValue.VariantVal(ci, p)
        }
        ENUM -> SchemaValue.EnumVal(r.i32())
        MAP -> {
            val n = r.i32()
            SchemaValue.MapVal((0 until n).map { get(r) to get(r) })
        }
        DATETIME -> SchemaValue.DatetimeVal(r.i64(), r.i32())
        UNIT -> SchemaValue.Unit_
        else -> error("SchemaValueBytes.decode: unknown tag $tag")
    }

    private class Writer {
        private val b = ArrayList<Byte>(64)
        fun u8(v: Int) {
            b.add(v.toByte())
        }
        fun i32(v: Int) {
            b.add((v ushr 24).toByte())
            b.add((v ushr 16).toByte())
            b.add((v ushr 8).toByte())
            b.add(v.toByte())
        }
        fun i64(v: Long) {
            for (s in 56 downTo 0 step 8) b.add((v ushr s).toByte())
        }
        fun str(s: String) {
            val e = s.encodeToByteArray()
            i32(e.size)
            e.forEach { b.add(it) }
        }
        fun toByteArray() = b.toByteArray()
    }
    private class Reader(val b: ByteArray) {
        var i = 0
        fun u8(): Int = b[i++].toInt() and 0xFF
        fun i32(): Int {
            var v = 0
            repeat(4) { v = (v shl 8) or u8() }
            return v
        }
        fun i64(): Long {
            var v = 0L
            repeat(8) { v = (v shl 8) or u8().toLong() }
            return v
        }
        fun str(): String {
            val n = i32()
            val s = b.decodeToString(i, i + n)
            i += n
            return s
        }
    }
}
