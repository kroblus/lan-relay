package com.kroblus.lanrelay

import android.app.Application
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.OpenableColumns
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.AtomicFile
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asStateFlow
import org.json.JSONArray
import org.json.JSONObject
import java.io.File
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

class RelayApp : Application() {
    lateinit var store: RelayStore
        private set
    override fun onCreate() { super.onCreate(); store = RelayStore(this) }
}

private class Vault(context: Context) {
    private val file = AtomicFile(File(context.filesDir, "identity-v2.enc"))
    private fun key(): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey("lanrelay-v2", null) as? SecretKey)?.let { return it }
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").apply {
            init(KeyGenParameterSpec.Builder("lanrelay-v2", KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM).setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE).build())
        }.generateKey()
    }
    fun read(): JSONObject {
        if (!file.baseFile.exists()) return JSONObject()
        val bytes = file.readFully()
        require(bytes.size > 28)
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, bytes.copyOfRange(0, 12)))
        return JSONObject(String(cipher.doFinal(bytes.copyOfRange(12, bytes.size)), Charsets.UTF_8))
    }
    fun write(json: JSONObject) {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, key())
        val bytes = cipher.iv + cipher.doFinal(json.toString().toByteArray(Charsets.UTF_8))
        val out = file.startWrite()
        try { out.write(bytes); file.finishWrite(out) } catch (error: Exception) { file.failWrite(out); throw error }
    }
}

class RelayStore(private val context: Context) {
    private val vault = Vault(context)
    private val queueFile = AtomicFile(File(context.filesDir, "transfers-v2.json"))
    val preferences = context.getSharedPreferences("preferences-v2", Context.MODE_PRIVATE)
    private val _session = MutableStateFlow<Session?>(null)
    private val _connections = MutableStateFlow<List<Session>>(emptyList())
    private val _pending = MutableStateFlow<PendingPair?>(null)
    private val _tasks = MutableStateFlow<List<TransferTask>>(emptyList())
    private val _metrics = MutableStateFlow<Map<String, TransferMetrics>>(emptyMap())
    private var lastJournalTime = elapsedMillis()
    private var journalOffsets: Map<String, Long> = emptyMap()
    val session = _session.asStateFlow()
    val connections = _connections.asStateFlow()
    val pending = _pending.asStateFlow()
    val tasks = _tasks.asStateFlow()
    val metrics = _metrics.asStateFlow()
    val notice = MutableStateFlow("")

    init {
        runCatching {
            val identity = vault.read()
            _session.value = identity.optJSONObject("session")?.let(Session::parse)
            _connections.value = identity.optJSONArray("connections")?.objects()?.map(Session::parse)
                ?: listOfNotNull(_session.value)
            _pending.value = identity.optJSONObject("pending")?.let(PendingPair::parse)
        }.onFailure { notice.value = "无法恢复旧连接，请重新配对；本地任务已保留。" }
        runCatching {
            if (queueFile.baseFile.exists()) {
                val array = JSONArray(String(queueFile.readFully(), Charsets.UTF_8))
                require(array.length() <= 500)
                _tasks.value = array.objects().map { TransferTask.parse(it).afterProcessRestart() }
            }
        }.onFailure { notice.value = "任务记录无法读取，原记录已保留。请导出诊断后重试。" }
    }

    @Synchronized fun setSession(session: Session?) {
        val known = if (session == null) _connections.value else (listOf(session) + _connections.value.filter { it.serverId != session.serverId }).take(10)
        persistIdentity(session, null, known)
        if (session != null) {
            commitTasks(_tasks.value.map { if (it.serverId == session.serverId && it.deviceId == session.deviceId) it.copy(origin = session.origin) else it })
        }
        _session.value = session
        _connections.value = known
        _pending.value = null
    }
    @Synchronized fun setPending(pending: PendingPair?) {
        persistIdentity(_session.value, pending)
        _pending.value = pending
    }
    private fun persistIdentity(session: Session?, pending: PendingPair?, connections: List<Session> = _connections.value) {
        vault.write(JSONObject().put("session", session?.json()).put("pending", pending?.json()).put("connections", JSONArray(connections.map { it.json() })))
    }
    @Synchronized fun update(id: String, transform: (TransferTask) -> TransferTask) {
        commitTasks(_tasks.value.map { if (it.id == id) transform(it) else it })
    }
    @Synchronized fun updateProgress(id: String, transform: (TransferTask) -> TransferTask) {
        val old = task(id) ?: return
        val next = transform(old)
        val value = _tasks.value.map { if (it.id == id) next else it }
        val now = elapsedMillis()
        // The remote upload offset is authoritative on every resume. This journal is a cache,
        // while task identity, source SHA, pause/cancel and completion are always durable.
        if (ProgressJournalPolicy.criticalChange(old, next) ||
            (next.status != "hashing" && ProgressJournalPolicy.checkpointDue(lastJournalTime, journalOffsets[id] ?: old.offset, now, next.offset))) {
            commitTasks(value)
        } else _tasks.value = value
    }
    @Synchronized fun setMetrics(id: String, metrics: TransferMetrics?) {
        _metrics.value = if (metrics == null) _metrics.value - id else _metrics.value + (id to metrics)
    }
    @Synchronized fun add(task: TransferTask) {
        require(_tasks.value.size < 500) { "任务记录已达 500 条，请先清理已完成记录。" }
        commitTasks(_tasks.value + task)
    }
    @Synchronized fun pauseAll(reason: String = "已暂停，点继续恢复。") {
        commitTasks(_tasks.value.map { if (it.active || it.status == "queued") it.copy(status = "paused", error = reason) else it })
    }
    @Synchronized fun clearFinished() {
        val retained = _tasks.value.filter { !it.clearable }
        commitTasks(retained)
        _metrics.value = _metrics.value.filterKeys { key -> retained.any { it.id == key } }
    }
    fun task(id: String) = _tasks.value.firstOrNull { it.id == id }
    private fun commitTasks(value: List<TransferTask>) {
        val bytes = JSONArray(value.map { it.json() }).toString().toByteArray(Charsets.UTF_8)
        val out = queueFile.startWrite()
        try { out.write(bytes); queueFile.finishWrite(out) } catch (error: Exception) { queueFile.failWrite(out); throw error }
        lastJournalTime = elapsedMillis()
        journalOffsets = value.associate { it.id to it.offset }
        _tasks.value = value
    }

    fun selectFile(uri: Uri, project: String, buildId: String = "", replaceId: String? = null): TransferTask {
        require(uri.scheme == "content") { "请选择系统文件选择器提供的文件。" }
        val current = session.value ?: throw IllegalStateException("请先连接电脑。")
        var name = "file"
        var size = -1L
        context.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)?.use { cursor ->
            if (cursor.moveToFirst()) {
                val n = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                val s = cursor.getColumnIndex(OpenableColumns.SIZE)
                if (n >= 0) name = cursor.getString(n) ?: "file"
                if (s >= 0 && !cursor.isNull(s)) size = cursor.getLong(s)
            }
        }
        runCatching { context.contentResolver.takePersistableUriPermission(uri, Intent.FLAG_GRANT_READ_URI_PERMISSION) }
        val persisted = context.contentResolver.persistedUriPermissions.any { it.uri == uri && it.isReadPermission }
        val task = if (replaceId != null) {
            val old = task(replaceId) ?: throw IllegalArgumentException("任务不存在。")
            TransferRules.validateIdentity(old, current)
            require(old.direction == "upload")
            if (size >= 0 && old.size >= 0) require(size == old.size) { "文件大小不同，请选择原始文件。" }
            old.copy(uri = uri.toString(), persistedUri = persisted, status = "queued", error = "")
        } else TransferTask(direction = "upload", origin = current.origin, serverId = current.serverId, deviceId = current.deviceId,
            uri = uri.toString(), name = TransferRules.safeName(name), mime = context.contentResolver.getType(uri) ?: "application/octet-stream",
            size = size, project = project.take(120), buildId = buildId, persistedUri = persisted)
        if (replaceId == null) add(task) else update(task.id) { task }
        return task
    }
}
