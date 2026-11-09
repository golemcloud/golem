package p27

import cloud.golem.BaseAgent
import cloud.golem.CustomSnapshot
import cloud.golem.MultipartSnapshot
import cloud.golem.MultipartSnapshotted
import cloud.golem.SnapshotPart
import cloud.golem.annotations.Agent
import cloud.golem.annotations.Endpoint

private const val VERSION = "v1"

@Agent(mount = "/snap-custom/{id}", description = "custom byte snapshot", snapshotting = "enabled")
class SnapCustom(val id: String) : BaseAgent(), CustomSnapshot {
    private var total = 0
    private var restoredFrom = "none"

    @Endpoint(post = "/inc")
    fun inc(by: Int): Int {
        total += by
        return total
    }

    @Endpoint(post = "/get")
    fun get(): String = "$VERSION id=$id total=$total restored=$restoredFrom"

    override fun saveSnapshot(): ByteArray = "$VERSION:$total".encodeToByteArray()

    override fun loadSnapshot(bytes: ByteArray) {
        val text = bytes.decodeToString()
        total = text.substringAfter(':').toInt()
        restoredFrom = text.substringBefore(':')
    }
}

@Agent(mount = "/snap-parts/{id}", description = "multipart snapshot", snapshotting = "enabled")
class SnapParts(val id: String) : BaseAgent(), MultipartSnapshotted {
    private val history = mutableListOf<Int>()
    private var restoredFrom = "none"

    @Endpoint(post = "/add")
    fun add(n: Int): Int {
        history += n
        return history.size
    }

    @Endpoint(post = "/get")
    fun get(): String = "$VERSION id=$id history=$history restored=$restoredFrom"

    override fun saveSnapshotParts() = MultipartSnapshot(
        mapOf(
            "meta" to SnapshotPart(VERSION.encodeToByteArray(), "text/plain"),
            "history" to SnapshotPart(history.joinToString(",").encodeToByteArray(), "application/octet-stream"),
        ),
    )

    override fun loadSnapshotParts(snapshot: MultipartSnapshot) {
        restoredFrom = snapshot.requirePart("meta", "text/plain").decodeToString()
        val text = snapshot.requirePart("history").decodeToString()
        history.clear()
        if (text.isNotEmpty()) history += text.split(",").map { it.toInt() }
    }
}
