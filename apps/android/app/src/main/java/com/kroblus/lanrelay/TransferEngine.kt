package com.kroblus.lanrelay

import android.content.ContentValues
import android.content.Context
import android.net.Uri
import android.os.Environment
import android.provider.MediaStore
import org.json.JSONObject
import java.io.File
import java.io.FileNotFoundException
import java.io.InputStream
import java.io.IOException
import java.io.RandomAccessFile
import java.security.MessageDigest
import java.util.concurrent.atomic.AtomicBoolean

internal class TransferPaused : Exception()
internal class SourceUnavailable(message: String) : Exception(message)
internal class DownloadIntegrityFailure(message: String) : Exception(message)

/** One engine processes one task. No file-sized allocation, even when a provider is non-seekable. */
class TransferEngine(private val context: Context, private val store: RelayStore, private val session: Session) {
    private val api = RelayApi(session.origin, session.token)
    private val stopped = AtomicBoolean(false)
    private var activeId = ""
    fun cancel() { stopped.set(true); api.cancel() }

    private fun checkpoint() {
        if (stopped.get() || store.task(activeId)?.status in setOf("paused", "cancelled")) throw TransferPaused()
    }
    private fun update(progressOnly: Boolean = false, transform: (TransferTask) -> TransferTask) {
        val guarded: (TransferTask) -> TransferTask = { old ->
            val updated = transform(old)
            if (old.status == "cancelled") updated.copy(status = old.status, cancelPending = old.cancelPending || (updated.direction == "upload" && updated.uploadId.isNotEmpty()))
            else if (old.status == "paused") updated.copy(status = old.status) else updated
        }
        if (progressOnly) store.updateProgress(activeId, guarded) else store.update(activeId, guarded)
    }

    fun run(task: TransferTask) {
        activeId = task.id
        store.setMetrics(task.id, null)
        try {
            TransferRules.validateIdentity(task, session)
            if (task.direction != "upload" && task.savedUri.isNotEmpty()) {
                // A process may have stopped while publishing a pending MediaStore file.
                context.contentResolver.delete(Uri.parse(task.savedUri), null, null)
                update { it.copy(savedUri = "") }
            }
            val hello = api.verifyServer(task.serverId)
            checkpoint()
            if (task.direction == "upload") upload(task, hello) else download(task)
        } catch (_: TransferPaused) {
            // Pause/cancel state was durably recorded by the UI/notification before interrupting I/O.
        } catch (error: Exception) {
            val current = store.task(task.id)
            if (current?.status !in setOf("paused", "cancelled")) {
                val status = if (task.direction == "upload" && (error is SourceUnavailable || error is SecurityException || error is FileNotFoundException)) "needs_file" else "failed"
                store.update(task.id) { it.copy(status = status, error = error.message?.take(300) ?: "传输中断，请重试。") }
            }
        } finally {
            val current = store.task(task.id)
            if (current?.status == "cancelled") {
                if (current.cancelPending) {
                    runCatching { TransferCancellation.synchronize(store, current, session) }
                        .onFailure { store.update(task.id) { it.copy(error = "本机已取消，电脑端取消尚未确认。连接后可重试取消。") } }
                }
                partialFile(task).delete()
            }
        }
    }

    private fun openSource(task: TransferTask): InputStream = try {
        context.contentResolver.openInputStream(Uri.parse(task.uri)) ?: throw SourceUnavailable("原文件无法读取，请重新选择。")
    } catch (_: SecurityException) { throw SourceUnavailable("文件读取授权已失效，请重新选择原文件。") }
      catch (_: FileNotFoundException) { throw SourceUnavailable("原文件已移动或不可用，请重新选择。") }

    private fun hashSource(task: TransferTask, maximum: Long): Pair<Long, String> {
        update { it.copy(status = "hashing", error = "正在校验原文件…") }
        val digest = MessageDigest.getInstance("SHA-256")
        val buffer = ByteArray(256 * 1024)
        var total = 0L
        var nextReport = 0L
        openSource(task).use { stream ->
            while (true) {
                checkpoint()
                val count = stream.read(buffer)
                if (count < 0) break
                if (count == 0) continue
                total += count
                require(total <= maximum) { "文件超过电脑允许的单文件大小。" }
                digest.update(buffer, 0, count)
                if (total >= nextReport) {
                    update(progressOnly = true) { it.copy(error = "正在校验 ${formatBytes(total)}…") }
                    nextReport = total + 8 * 1024 * 1024
                }
            }
        }
        return total to digest.digest().toHex()
    }

    private fun upload(original: TransferTask, hello: Hello) {
        val (size, sha) = hashSource(original, hello.maxFileBytes)
        try { TransferRules.validateSource(if (original.sha256.isBlank()) -1 else original.size, original.sha256, size, sha) }
        catch (error: IllegalArgumentException) { throw SourceUnavailable(error.message ?: "请重新选择原始文件。") }
        update { it.copy(size = size, sha256 = sha, error = "", status = "uploading") }
        checkpoint()
        var task = store.task(activeId)!!
        var remote = if (task.uploadId.isEmpty()) {
            val payload = JSONObject().put("name", task.name).put("size", size).put("mime", task.mime).put("sha256", sha)
                .put("project", task.project).put("request_id", task.requestId)
            if (task.buildId.isNotBlank()) payload.put("build_id", task.buildId)
            RemoteUpload.parse(api.json("/api/v2/uploads", "POST", payload))
        } else api.upload(task.uploadId)
        require(remote.size == size && remote.sha256 == sha) { "电脑上的续传任务与原文件不一致。" }
        update { it.copy(uploadId = remote.id, offset = remote.offset) }
        checkpoint()
        require(remote.status == "uploading" || remote.status == "complete") { "电脑任务已失效，请取消此任务并重新发送。" }
        val buffer = ByteArray(minOf(hello.chunkSize, remote.chunkSize))
        var conflicts = 0
        while (remote.status != "complete" && remote.offset < size) {
            checkpoint()
            // Reopening after a conflict also supports content providers without seekable descriptors.
            openSource(task).use { input ->
                skipExactly(input, remote.offset)
                // A provider may have to read to seek; source positioning is not network speed.
                var telemetry = NetworkTelemetry(size, remote.offset, elapsedMillis())
                store.setMetrics(activeId, TransferMetrics())
                while (remote.offset < size) {
                    checkpoint()
                    val count = readChunk(input, buffer, minOf(buffer.size.toLong(), size - remote.offset).toInt())
                    require(count > 0) { "原文件在传输期间变短，请重新选择原始文件。" }
                    try {
                        val before = remote.offset
                        var lastWritten = 0L
                        val next = api.sendChunk(remote.id, before, buffer, count) { written ->
                            checkpoint()
                            val now = elapsedMillis()
                            // OkHttp may retry a request stream before returning a response.
                            // Restart display sampling without advancing any confirmed offset.
                            if (written < lastWritten) telemetry = NetworkTelemetry(size, before, now)
                            lastWritten = written
                            telemetry.observe(before + written, now)?.let { store.setMetrics(activeId, it) }
                        }
                        require(next.offset == before + count && next.size == size && next.sha256 == sha) { "电脑返回了不一致的已确认进度。" }
                        remote = next
                        update(progressOnly = true) { it.copy(offset = remote.offset, error = "") }
                        conflicts = 0
                    } catch (error: ApiFailure) {
                        if (error.status != 409 || ++conflicts > 3) throw error
                        remote = api.upload(remote.id)
                        require(remote.size == size && remote.sha256 == sha && remote.status in setOf("uploading", "complete")) { "续传任务已经改变。" }
                        update { it.copy(offset = remote.offset) }
                        break
                    }
                }
            }
        }
        checkpoint()
        update { it.copy(status = "verifying", error = "等待电脑完成 SHA-256 校验…") }
        val item = FeedItem.parse(api.json("/api/v2/uploads/${LinkPolicy.id(remote.id)}/complete", "POST"))
        require(item.size == size && item.sha256 == sha && item.kind == "file") { "电脑返回的完成文件与原文件不一致。" }
        update { it.copy(itemId = item.id, offset = size, status = "complete", error = "SHA-256 一致，已保存到电脑。") }
    }

    private fun partialFile(task: TransferTask): File {
        val directory = File(context.filesDir, "transfer-parts").apply { mkdirs() }
        return File(directory, "${LinkPolicy.id(task.id)}.part")
    }

    private fun download(task: TransferTask) {
        val export = task.direction == "export"
        val path = if (export) "/api/v2/feedback/${LinkPolicy.id(task.itemId)}/export" else "/api/v2/files/${LinkPolicy.id(task.itemId)}"
        if (!export) require(task.size >= 0 && task.sha256.matches(Regex("[0-9a-f]{64}"))) { "文件缺少可验证的长度或 SHA-256。" }
        val part = partialFile(task)
        val maximum = if (export) 4L * 1024 * 1024 else task.size
        var retries = 0
        while (true) {
            checkpoint()
            var offset = if (!export && part.exists() && part.length() <= task.size) part.length() else 0L
            update { it.copy(status = "downloading", offset = offset, error = "") }
            var telemetry = NetworkTelemetry(task.size, offset, elapsedMillis())
            val headers = TransferRules.downloadHeaders(offset, task.sha256)
            try {
            if (retries > 0) network { api.verifyServer(task.serverId) }
            if (offset < maximum || export || !part.exists()) network { api.execute(path, headers = headers) }.use { response ->
                if (!response.isSuccessful) api.parseResponse(response)
                require(response.header("Content-Disposition").orEmpty().startsWith("attachment;", ignoreCase = true)) { "电脑返回的不是文件附件。" }
                if (!export) require(response.header("ETag") == "\"sha256-${task.sha256}\"") { "电脑上的文件身份发生变化，请刷新后重新选择。" }
                if (offset > 0 && response.code == 200) {
                    offset = 0
                    telemetry = NetworkTelemetry(task.size, 0, elapsedMillis())
                }
                if (offset > 0) {
                    val range = Regex("bytes (\\d+)-(\\d+)/(\\d+)").matchEntire(response.header("Content-Range").orEmpty())
                    require(response.code == 206 && range != null && range.groupValues[1].toLong() == offset && range.groupValues[3].toLong() == task.size) { "电脑返回的下载范围不正确。" }
                } else require(response.code == 200) { "电脑返回了意外的文件范围。" }
                val body = response.body ?: throw IllegalStateException("下载响应为空。")
                RandomAccessFile(part, "rw").use { output ->
                    output.setLength(offset)
                    output.seek(offset)
                    try { body.byteStream().use { input ->
                        val buffer = ByteArray(256 * 1024)
                        var checkpointBytes = offset
                        while (true) {
                            checkpoint()
                            val count = network { input.read(buffer) }
                            if (count < 0) break
                            if (count == 0) continue
                            require(offset + count <= maximum) { "下载内容超过声明的文件大小。" }
                            // Activity is based on received bytes, not the much less frequent
                            // 2 MiB durability checkpoint. This is an in-memory UI signal only.
                            telemetry.observe(offset + count, elapsedMillis())?.let { store.setMetrics(activeId, it) }
                            output.write(buffer, 0, count)
                            offset += count
                            if (offset - checkpointBytes >= 2 * 1024 * 1024) {
                                output.fd.sync()
                                update(progressOnly = true) { it.copy(offset = offset) }
                                checkpointBytes = offset
                            }
                        }
                    } } finally { output.fd.sync() }
                }
            }
            if (!export && part.length() != task.size) throw DownloadTransportFailure(IOException("连接提前结束，正在恢复下载。"))
            break
            } catch (error: Exception) {
                checkpoint()
                val wait = DownloadRetryPolicy.delayMillis(error, retries) ?: throw error
                retries++
                store.setMetrics(activeId, null)
                update { it.copy(status = "retrying", offset = if (part.exists()) part.length() else 0L,
                    error = "连接暂时中断，正在自动续传（$retries / 3）…") }
                // Short cancellable waits retain pause/cancel responsiveness during backoff.
                val until = elapsedMillis() + wait
                while (elapsedMillis() < until) { checkpoint(); Thread.sleep(minOf(100, until - elapsedMillis()).coerceAtLeast(1)) }
            }
        }
        checkpoint()
        if (!export) require(part.length() == task.size) { "下载尚未完整，请继续恢复。" }
        update { it.copy(status = "verifying", offset = part.length(), error = "正在校验并保存文件…") }
        // Verify and copy in one pass. The public MediaStore entry stays IS_PENDING
        // until both exact length and SHA match, so unverified bytes are never published.
        val (saved, hash) = try { saveToDownloads(part, task) }
        catch (error: DownloadIntegrityFailure) {
            part.delete()
            update { it.copy(offset = 0) }
            throw error
        }
        // saveToDownloads returned only after full verification and publication.
        // Do not leave a published file attached to a late "cancelled" record that
        // can be cleared, or a "paused" record whose completed part would disappear.
        val recorded = store.finishSavedDownload(activeId, saved.toString(), part.length(), hash)
        // A cancelled record may already have been cleared before this atomic
        // finalization. Remove only this newly published file in that case. A
        // record cleared AFTER completion must leave its saved file untouched.
        if (!recorded) context.contentResolver.delete(saved, null, null)
        part.delete()
    }

    private inline fun <T> network(block: () -> T): T = try { block() }
        catch (error: ApiFailure) { throw error }
        catch (error: IOException) { throw DownloadTransportFailure(error) }

    private fun saveToDownloads(file: File, task: TransferTask): Pair<Uri, String> {
        val resolver = context.contentResolver
        val values = ContentValues().apply {
            put(MediaStore.Downloads.DISPLAY_NAME, "${System.currentTimeMillis()}-${TransferRules.safeName(task.name)}")
            put(MediaStore.Downloads.MIME_TYPE, FileOpenPolicy.mime(task.name, task.mime))
            put(MediaStore.Downloads.RELATIVE_PATH, Environment.DIRECTORY_DOWNLOADS + "/LANRelay")
            put(MediaStore.Downloads.IS_PENDING, 1)
        }
        val uri = resolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values) ?: throw IllegalStateException("无法创建下载文件。")
        update { it.copy(savedUri = uri.toString()) }
        try {
            val result = resolver.openOutputStream(uri, "w")?.use { output ->
                file.inputStream().use { input -> TransferIo.copyAndHash(input, output, ::checkpoint) }
            } ?: throw IllegalStateException("无法写入下载目录。")
            if (task.direction != "export" && (result.first != task.size || result.second != task.sha256)) {
                throw DownloadIntegrityFailure("SHA-256 或文件长度不一致，损坏的临时下载已移除，请重试。")
            }
            checkpoint()
            require(resolver.update(uri, ContentValues().apply { put(MediaStore.Downloads.IS_PENDING, 0) }, null, null) == 1) { "文件保存未完成。" }
            return uri to result.second
        } catch (error: Exception) {
            resolver.delete(uri, null, null)
            update { it.copy(savedUri = "") }
            throw error
        }
    }

    private fun skipExactly(input: InputStream, bytes: Long) {
        var remaining = bytes
        val scratch = ByteArray(64 * 1024)
        while (remaining > 0) {
            checkpoint()
            val n = input.read(scratch, 0, minOf(remaining, scratch.size.toLong()).toInt())
            require(n > 0) { "原文件长度不足，无法续传。" }
            remaining -= n
        }
    }
    private fun readChunk(input: InputStream, buffer: ByteArray, needed: Int): Int {
        var count = 0
        while (count < needed) {
            checkpoint()
            val n = input.read(buffer, count, needed - count)
            if (n < 0) break
            if (n == 0) continue
            count += n
        }
        return count
    }
}

internal fun ByteArray.toHex() = joinToString("") { "%02x".format(it.toInt() and 255) }
