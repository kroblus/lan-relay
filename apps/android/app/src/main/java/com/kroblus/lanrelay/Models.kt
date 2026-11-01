package com.kroblus.lanrelay

import org.json.JSONArray
import org.json.JSONObject
import java.util.UUID

internal fun JSONObject.string(key: String, fallback: String = ""): String =
    if (isNull(key)) fallback else optString(key, fallback)
internal fun JSONArray.objects(): List<JSONObject> = (0 until length()).map { getJSONObject(it) }

data class Session(val origin: String, val serverId: String, val serverName: String, val deviceId: String, val token: String) {
    fun json() = JSONObject().put("origin", origin).put("server_id", serverId).put("server_name", serverName)
        .put("device_id", deviceId).put("token", token)
    companion object { fun parse(j: JSONObject) = Session(j.getString("origin"), j.getString("server_id"), j.string("server_name"), j.getString("device_id"), j.getString("token")) }
}

data class PendingPair(val origin: String, val serverId: String, val serverName: String, val id: String, val claimToken: String, val expiresAt: Long, val verificationCode: String = "", val cancelRequested: Boolean = false) {
    fun json() = JSONObject().put("origin", origin).put("server_id", serverId).put("server_name", serverName)
        .put("id", id).put("claim_token", claimToken).put("expires_at", expiresAt).put("verification_code", verificationCode).put("cancel_requested", cancelRequested)
    companion object { fun parse(j: JSONObject) = PendingPair(j.getString("origin"), j.getString("server_id"), j.string("server_name"), j.getString("id"), j.getString("claim_token"), j.getLong("expires_at"), j.string("verification_code"), j.optBoolean("cancel_requested")) }
}

data class Hello(val protocol: Int, val serverId: String, val name: String, val maxFileBytes: Long, val chunkSize: Int, val capabilities: Set<String> = emptySet()) {
    companion object {
        fun parse(j: JSONObject): Hello {
            require(j.getInt("protocol_version") == 2) { "电脑协议不兼容，请更新电脑或手机应用。" }
            val size = j.getLong("max_file_bytes")
            val chunk = j.getInt("chunk_size")
            require(size > 0 && chunk in 1..(16 * 1024 * 1024)) { "电脑返回了无效传输限制。" }
            val capabilities = j.optJSONArray("capabilities")
            return Hello(2, j.getString("server_id"), j.getString("name"), size, minOf(chunk, 8 * 1024 * 1024),
                if (capabilities == null) emptySet() else (0 until capabilities.length()).map { capabilities.getString(it) }.toSet())
        }
    }
}

data class FeedItem(
    val id: String, val kind: String, val createdAt: Long, val name: String, val text: String,
    val project: String, val mime: String, val size: Long, val sha256: String,
    val buildId: String, val attachments: List<String>, val note: String, val testStatus: String,
    val apkPackage: String = "", val apkVersion: String = ""
) {
    val isApk get() = kind == "file" && name.endsWith(".apk", ignoreCase = true)
    companion object {
        fun parse(j: JSONObject): FeedItem {
            val apk = j.optJSONObject("apk")
            val a = j.optJSONArray("attachment_ids") ?: JSONArray()
            return FeedItem(j.getString("id"), j.getString("kind"), j.optLong("created_at"), j.string("name"),
                j.string("text"), j.string("project"), j.string("mime", "application/octet-stream"), j.optLong("size"),
                j.string("sha256"), j.string("build_id"), (0 until a.length()).map { a.getString(it) },
                j.string("note"), j.string("test_status", "unverified"), apk?.string("package_name") ?: "", apk?.string("version_name") ?: "")
        }
    }
}

data class RemoteUpload(val id: String, val offset: Long, val size: Long, val chunkSize: Int, val status: String, val sha256: String) {
    companion object {
        fun parse(j: JSONObject): RemoteUpload {
            val value = RemoteUpload(j.getString("id"), j.getLong("offset"), j.getLong("size"), j.getInt("chunk_size"), j.getString("status"), j.getString("sha256"))
            require(value.offset in 0..value.size && value.chunkSize in 1..(16 * 1024 * 1024)) { "电脑返回了无效续传位置。" }
            return value.copy(chunkSize = minOf(value.chunkSize, 8 * 1024 * 1024))
        }
    }
}

data class TransferTask(
    val id: String = UUID.randomUUID().toString(), val direction: String, val origin: String,
    val serverId: String, val deviceId: String, val uri: String = "", val name: String,
    val mime: String = "application/octet-stream", val size: Long = -1, val sha256: String = "",
    val project: String = "", val buildId: String = "", val uploadId: String = "", val itemId: String = "",
    val requestId: String = UUID.randomUUID().toString(), val offset: Long = 0,
    val status: String = "queued", val error: String = "", val savedUri: String = "", val persistedUri: Boolean = false,
    val cancelPending: Boolean = false
) {
    val active get() = status in setOf("hashing", "uploading", "downloading", "retrying", "verifying", "saving")
    val terminal get() = status == "complete" || status == "cancelled"
    val clearable get() = terminal && !cancelPending
    fun afterProcessRestart() = if (active || status == "queued") copy(status = "paused", error = "应用已重新启动，点继续恢复任务。") else this
    fun json() = JSONObject().put("id", id).put("direction", direction).put("origin", origin).put("server_id", serverId)
        .put("device_id", deviceId).put("uri", uri).put("name", name).put("mime", mime).put("size", size).put("sha256", sha256)
        .put("project", project).put("build_id", buildId).put("upload_id", uploadId).put("item_id", itemId).put("request_id", requestId)
        .put("offset", offset).put("status", status).put("error", error).put("saved_uri", savedUri).put("persisted_uri", persistedUri).put("cancel_pending", cancelPending)
    companion object {
        fun parse(j: JSONObject) = TransferTask(j.getString("id"), j.getString("direction"), j.getString("origin"),
            j.getString("server_id"), j.getString("device_id"), j.string("uri"), j.getString("name"), j.string("mime", "application/octet-stream"),
            j.optLong("size", -1), j.string("sha256"), j.string("project"), j.string("build_id"), j.string("upload_id"), j.string("item_id"),
            j.getString("request_id"), j.optLong("offset"), j.string("status", "paused"), j.string("error"), j.string("saved_uri"), j.optBoolean("persisted_uri"), j.optBoolean("cancel_pending"))
    }
}

object TransferRules {
    fun resume(task: TransferTask, session: Session): TransferTask {
        validateIdentity(task, session)
        return if (task.status in setOf("paused", "failed")) task.copy(status = "queued", error = "") else task
    }
    /** Once a verified file is publicly saved, report that durable outcome even
     * if pause/cancel raced the final metadata update. No installation is inferred. */
    fun savedDownload(task: TransferTask, uri: String, size: Long, sha256: String): TransferTask {
        require(task.direction != "upload" && uri.startsWith("content://"))
        if (task.direction != "export") require(size == task.size && sha256 == task.sha256)
        return task.copy(status = "complete", savedUri = uri, size = size, offset = size, sha256 = sha256,
            cancelPending = false, error = if (task.direction == "export") "反馈已导出到下载目录。" else "SHA-256 一致，文件已完成保存。")
    }
    fun downloadHeaders(offset: Long, sha256: String): Map<String, String> = if (offset > 0)
        mapOf("Range" to "bytes=$offset-", "If-Range" to "\"sha256-$sha256\"") else emptyMap()
    fun validateIdentity(task: TransferTask, session: Session) {
        require(task.serverId == session.serverId && task.deviceId == session.deviceId && task.origin == session.origin) {
            "此任务属于另一台电脑或旧设备身份，请连接原电脑后继续。"
        }
    }
    fun validateSource(expectedSize: Long, expectedHash: String, size: Long, sha: String) {
        require(expectedSize < 0 || expectedSize == size) { "重选文件大小不同，请选择原始文件。" }
        require(expectedHash.isBlank() || expectedHash == sha) { "文件 SHA-256 不同，请选择原始文件。" }
    }
    fun markCancelled(task: TransferTask): TransferTask {
        val pending = task.cancelPending || (task.direction == "upload" && (task.uploadId.isNotEmpty() || task.sha256.isNotEmpty() || task.status in setOf("uploading", "verifying")))
        return task.copy(status = "cancelled", cancelPending = pending, error = if (pending) "本机已取消，电脑端取消尚未确认。连接原电脑后可重试取消。" else "已取消")
    }
    fun safeName(name: String): String = name.replace(Regex("[\\\\/\\p{Cntrl}]"), "_").take(180).ifBlank { "file" }
}

fun formatBytes(value: Long): String = when {
    value < 0 -> "大小待确认"
    value < 1024 -> "$value B"
    value < 1024 * 1024 -> "%.1f KiB".format(value / 1024.0)
    value < 1024L * 1024 * 1024 -> "%.1f MiB".format(value / (1024.0 * 1024))
    else -> "%.2f GiB".format(value / (1024.0 * 1024 * 1024))
}
