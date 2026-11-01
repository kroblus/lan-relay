package com.kroblus.lanrelay

import java.util.UUID
import java.util.concurrent.atomic.AtomicBoolean

internal class DraftSubmissionGate {
    private val held = AtomicBoolean(false)
    fun acquire(): Boolean = held.compareAndSet(false, true)
    fun release() { held.set(false) }
}

internal object DraftTaskIdentity {
    fun bind(task: TransferTask, draftId: String): TransferTask {
        val identity = UUID.nameUUIDFromBytes(listOf("draft-v1", draftId, task.serverId, task.deviceId, task.uri)
            .joinToString("\u0000").toByteArray(Charsets.UTF_8)).toString()
        return task.copy(id = identity, requestId = identity)
    }
    fun sameSource(a: TransferTask, b: TransferTask): Boolean = a.serverId == b.serverId && a.deviceId == b.deviceId &&
        a.uri == b.uri && a.name == b.name && a.mime == b.mime && a.project == b.project && a.requestId == b.requestId &&
        (a.size < 0 || b.size < 0 || a.size == b.size)
}
