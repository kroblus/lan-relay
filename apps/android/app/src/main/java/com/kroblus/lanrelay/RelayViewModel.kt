package com.kroblus.lanrelay

import android.app.Application
import android.net.Uri
import android.content.Intent
import android.provider.OpenableColumns
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject

class RelayViewModel(application: Application) : AndroidViewModel(application) {
    val store = (application as RelayApp).store
    private val discoveryId = store.preferences.getString("discovery_peer_id", null) ?: java.util.UUID.randomUUID().toString().also {
        store.preferences.edit().putString("discovery_peer_id", it).apply()
    }
    val discovery = LanDiscovery(discoveryId) { store.preferences.getString("device_name", "我的手机").orEmpty() }
    val items = MutableStateFlow<List<FeedItem>>(emptyList())
    val busy = MutableStateFlow(false)
    val online = MutableStateFlow(false)
    val moreAvailable = MutableStateFlow(false)
    val incomingAddress = MutableStateFlow("")
    val sharedFiles = MutableStateFlow<List<Uri>>(emptyList())
    val sharedText = MutableStateFlow("")
    val stagedNames = MutableStateFlow<Map<String, String>>(emptyMap())
    private val visible = MutableStateFlow(false)
    private val refreshLock = Mutex()
    private val draftSubmission = DraftSubmissionGate()
    private var nextCursor: String? = null
    private var feedIdentity = ""

    init {
        viewModelScope.launch {
            visible.collectLatest { active ->
                if (active) while (true) {
                    store.pending.value?.let { pollPair(it) }
                    if (store.session.value != null) refresh()
                    delay(4000)
                }
            }
        }
    }
    fun visible(value: Boolean) { visible.value = value }
    fun startDiscovery() { if (visible.value) discovery.start() }
    fun stopDiscovery() { discovery.close() }
    override fun onCleared() { discovery.close(); super.onCleared() }
    fun message(value: String) { store.notice.value = value }

    fun stageFiles(uris: List<Uri>) = action {
        val selected = (sharedFiles.value + uris).distinct()
        require(selected.size <= 100) { "单次最多添加 100 个文件。" }
        val resolver = getApplication<Application>().contentResolver
        val names = selected.associate { uri ->
            require(uri.scheme == "content") { "请选择系统文件选择器提供的文件。" }
            runCatching { resolver.takePersistableUriPermission(uri, Intent.FLAG_GRANT_READ_URI_PERMISSION) }
            var name = "文件"
            resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { cursor ->
                if (cursor.moveToFirst()) name = cursor.getString(0).orEmpty().ifBlank { "文件" }
            }
            uri.toString() to name
        }
        stagedNames.value = names
        sharedFiles.value = selected
    }

    fun sendDraft(text: String, project: String, requestId: String, selectedFiles: List<Uri>, expectedSession: Session,
        done: () -> Unit, started: () -> Unit) {
        // Acquire before scheduling the coroutine; a second tap can arrive before
        // Compose sees busy=true. Pause/cancel operations never share this gate.
        if (!draftSubmission.acquire()) return
        action { try {
        val files = selectedFiles.toList()
        require(text.isNotBlank() || files.isNotEmpty()) { "添加文件，或写一段文字后发送。" }
        val session = expectedSession
        requireSession(session)
        refreshLock.withLock {
            requireSession(session)
            // Prepare the entire selection before mutating the durable queue, so a
            // revoked file URI cannot leave half the selection queued twice.
            val prepared = files.map { DraftTaskIdentity.bind(store.prepareFile(it, project), requestId) }
            val newCount = prepared.count { incoming -> store.tasks.value.none { it.id == incoming.id } }
            require(store.tasks.value.size + newCount <= 500) { "请先清理已完成记录，再发送更多文件。" }
            if (text.isNotBlank()) verifiedApi(session).json("/api/v2/text", "POST",
                JSONObject().put("text", text).put("project", project).put("request_id", requestId))
            requireSession(session)
            if (prepared.isNotEmpty()) store.addAll(prepared)
        }
        withContext(Dispatchers.Main) {
            requireSession(session)
            sharedFiles.value = sharedFiles.value.filterNot { it in files }
            done()
            if (files.isNotEmpty()) started()
        }
        message(if (files.isNotEmpty()) "已开始发送 ${files.size} 个文件${if (text.isNotBlank()) "，文字已送达" else ""}。" else "文字已送达。")
        refresh()
        } finally { draftSubmission.release() } }
    }

    private fun identity(session: Session) = session.serverId + ":" + session.deviceId
    private fun requireSession(session: Session) {
        require(store.session.value == session) { "连接已改变，请核对目标电脑后重新操作。" }
    }
    private fun verifiedApi(session: Session): RelayApi {
        requireSession(session)
        return RelayApi.verified(session).also { requireSession(session) }
    }
    private fun requireFeed(session: Session, ids: Collection<String>) {
        requireSession(session)
        require(feedIdentity == identity(session) && ids.all { id -> items.value.any { it.id == id } }) {
            "这条内容不属于当前电脑的列表，请刷新后重新选择。"
        }
    }
    private suspend fun activateSession(session: Session?, expectedPair: String? = null): Boolean = refreshLock.withLock {
        if (expectedPair != null && (store.pending.value?.id != expectedPair || store.pending.value?.cancelRequested == true)) return@withLock false
        if (store.session.value != session) store.pauseAll("连接已切换，原电脑的任务和断点保留。")
        // Clear the old computer's actions before publishing the new authenticated identity.
        items.value = emptyList()
        nextCursor = null
        moreAvailable.value = false
        feedIdentity = ""
        online.value = false
        store.setSession(session)
        true
    }

    fun connect(address: String, name: String, code: String) = action {
        require(store.pending.value == null) { "正在等待电脑确认，请先完成或取消当前连接。" }
        val link = LinkPolicy.parse(address)
        require(name.trim().isNotEmpty() && name.length <= 80) { "请填写 1–80 个字符的设备名称。" }
        store.pauseAll("连接已切换，请回到原电脑后继续旧任务。")
        val api = RelayApi(link.origin)
        val hello = api.hello()
        val known = store.connections.value.firstOrNull { it.serverId == hello.serverId }
        if (known != null) {
            try {
                val candidate = known.copy(origin = link.origin, serverName = hello.name)
                RelayApi.verified(candidate).json("/api/v2/state")
                activateSession(candidate)
                store.preferences.edit().putString("last_origin", link.origin).apply()
                message("已重新连接 ${hello.name}，原设备身份与断点保留。")
                return@action
            } catch (error: ApiFailure) {
                if (error.status != 401 && error.status != 403) throw error
            }
        }
        if (link.inviteToken.isEmpty() && code.isBlank() && "discovery_pairing" in hello.capabilities) {
            requestDiscoveryPair(api, hello, name)
            return@action
        }
        if (link.inviteToken.isEmpty()) require(code.matches(Regex("[0-9]{6}"))) { "旧版电脑需要当前六位连接码，更新电脑端后可免输入。" }
        val payload = JSONObject().put("name", name.trim())
        if (link.inviteToken.isNotEmpty()) payload.put("token", link.inviteToken) else payload.put("code", code)
        val result = api.json("/api/v2/pair/requests", "POST", payload, authenticated = false)
        store.setPending(PendingPair(link.origin, hello.serverId, hello.name, result.getString("id"), result.getString("claim_token"), result.getLong("expires_at")))
        store.preferences.edit().putString("last_origin", link.origin).putString("device_name", name.trim()).apply()
        message("配对请求已发送，请在电脑窗口批准。")
    }

    fun connectDiscovered(peer: NearbyDesktop, name: String) = action {
        require(name.trim().isNotEmpty() && name.length <= 80) { "请填写 1–80 个字符的设备名称。" }
        require(store.pending.value == null) { "正在等待电脑确认，请先完成或取消当前连接。" }
        val api = RelayApi(peer.origin)
        val hello = api.verifyServer(peer.id)
        store.connections.value.firstOrNull { it.serverId == hello.serverId }?.let { known ->
            val candidate = known.copy(origin = peer.origin, serverName = hello.name)
            try {
                RelayApi.verified(candidate).json("/api/v2/state")
                store.pauseAll("连接已切换，原任务保留。")
                activateSession(candidate)
                message("已连接 ${hello.name}，无需重复验证。")
                return@action
            } catch (error: ApiFailure) { if (error.status !in setOf(401, 403)) throw error }
        }
        require("discovery_pairing" in hello.capabilities) { "这台电脑需要更新至 0.5 版本；也可以展开手动连接。" }
        requestDiscoveryPair(api, hello, name)
    }

    private fun requestDiscoveryPair(api: RelayApi, hello: Hello, name: String) {
        val result = api.json("/api/v2/pair/discovery", "POST", JSONObject().put("name", name.trim()).put("peer_id", discoveryId)
            .put("server_id", hello.serverId), authenticated = false)
        val verification = result.getString("verification_code")
        require(verification.matches(Regex("[0-9]{6}"))) { "电脑返回的验证信息不正确。" }
        store.setPending(PendingPair(api.origin, hello.serverId, hello.name, result.getString("id"), result.getString("claim_token"), result.getLong("expires_at"), verification))
        store.preferences.edit().putString("last_origin", api.origin).putString("device_name", name.trim()).apply()
        message("请核对两端显示的验证数字，再在电脑点信任。无需输入连接码。")
    }

    private suspend fun pollPair(pending: PendingPair) {
        if (pending.expiresAt < System.currentTimeMillis()) { store.setPending(null); message("连接请求已过期，请在附近设备中重新选择电脑。"); return }
        if (pending.cancelRequested) {
            withContext(Dispatchers.IO) { cancelPendingPair(pending) }
            return
        }
        runCatching {
            withContext(Dispatchers.IO) {
                val api = RelayApi(pending.origin)
                api.verifyServer(pending.serverId)
                val result = api.json("/api/v2/pair/claim", "POST", JSONObject().put("id", pending.id).put("claim_token", pending.claimToken), authenticated = false)
                if (store.pending.value?.id != pending.id || store.pending.value?.cancelRequested == true) return@withContext
                when (result.getString("status")) {
                    "approved" -> {
                        val device = result.getJSONObject("device")
                        if (activateSession(Session(pending.origin, pending.serverId, pending.serverName, device.getString("id"), result.getString("token")), pending.id)) {
                            message("已连接 ${pending.serverName}。")
                        }
                    }
                    "rejected", "expired" -> { store.setPending(null); message("电脑未批准或邀请已过期，请重新配对。") }
                }
            }
        }.onFailure { message(it.message ?: "等待电脑响应，稍后会重试。") }
    }

    fun cancelPairing() = action {
        val pending = refreshLock.withLock {
            store.pending.value?.copy(cancelRequested = true)?.also(store::setPending)
        } ?: return@action
        cancelPendingPair(pending)
    }
    fun continuePairing() = action {
        refreshLock.withLock { store.pending.value?.copy(cancelRequested = false)?.let(store::setPending) }
        message("正在读取电脑的连接结果。")
    }
    private fun cancelPendingPair(pending: PendingPair) {
        try {
            val api = RelayApi(pending.origin)
            api.verifyServer(pending.serverId)
            val result = api.json("/api/v2/pair/cancel", "POST", JSONObject().put("id", pending.id).put("claim_token", pending.claimToken), authenticated = false)
            require(result.getString("status") in setOf("rejected", "expired")) { "电脑仍在处理此请求，请稍后再试。" }
        } catch (error: Exception) {
            if (!(error is ApiFailure && error.status == 404 && pending.verificationCode.isBlank())) {
                message(if (error is ApiFailure && error.code == "PAIR_ALREADY_APPROVED") "电脑已批准，但手机不会自动连接。可选择继续连接，或在电脑撤销这台设备。" else "暂时无法确认取消。已停止自动连接，电脑恢复在线后会重试取消。")
                return
            }
        }
        if (store.pending.value?.id == pending.id) store.setPending(null)
        message("已取消连接请求，可以重新选择设备。")
    }
    fun disconnect() = action {
        store.pauseAll("已断开连接，任务保留。")
        activateSession(null)
        message("已断开。可信电脑记录与断点保留，可在连接页重新连接。")
    }

    suspend fun refresh() = refreshLock.withLock {
        val session = store.session.value ?: return@withLock
        try {
            val result = withContext(Dispatchers.IO) {
                verifiedApi(session).json("/api/v2/items?limit=50")
            }
            if (store.session.value != session) return@withLock
            val incoming = result.getJSONArray("items").objects().map(FeedItem::parse)
            val currentIdentity = identity(session)
            if (feedIdentity != currentIdentity) {
                feedIdentity = currentIdentity
                items.value = incoming
                nextCursor = result.string("next_cursor").ifBlank { null }
            } else {
                items.value = (incoming + items.value).distinctBy { it.id }.sortedWith(compareByDescending<FeedItem> { it.createdAt }.thenByDescending { it.id })
                if (nextCursor == null && items.value.size <= 50) nextCursor = result.string("next_cursor").ifBlank { null }
            }
            moreAvailable.value = nextCursor != null
            online.value = true
        } catch (error: Exception) {
            online.value = false
            if (error is ApiFailure && error.status == 401) message("电脑已撤销连接或会话失效，请重新配对。现有任务已保留。")
        }
    }

    fun refreshNow() { viewModelScope.launch { refresh() } }
    fun loadMore() = action {
        val session = store.session.value ?: throw IllegalStateException("请先连接电脑。")
        refreshLock.withLock {
            requireFeed(session, emptyList())
            val cursor = nextCursor ?: return@withLock
            val encoded = java.net.URLEncoder.encode(cursor, "UTF-8")
            val result = verifiedApi(session).json("/api/v2/items?limit=50&before=$encoded")
            requireSession(session)
            val older = result.getJSONArray("items").objects().map(FeedItem::parse)
            items.value = (items.value + older).distinctBy { it.id }
            nextCursor = result.string("next_cursor").ifBlank { null }
            moreAvailable.value = nextCursor != null
        }
    }

    fun sendText(text: String, project: String, requestId: String, done: () -> Unit) = action {
        require(text.isNotBlank()) { "请先输入文字。" }
        val session = store.session.value ?: throw IllegalStateException("请先连接电脑。")
        refreshLock.withLock {
            verifiedApi(session).json("/api/v2/text", "POST", JSONObject().put("text", text).put("project", project).put("request_id", requestId))
        }
        withContext(Dispatchers.Main) { requireSession(session); done() }
        message("文字已发送。")
        refresh()
    }

    fun sendFeedback(buildId: String, text: String, attachments: Set<String>, requestId: String, done: () -> Unit) = action {
        require(text.isNotBlank()) { "请写下复现步骤、预期和实际表现。" }
        val session = store.session.value ?: throw IllegalStateException("请先连接电脑。")
        val payload = JSONObject().put("build_id", buildId).put("text", text).put("attachment_ids", JSONArray(attachments.toList())).put("request_id", requestId)
        refreshLock.withLock {
            requireFeed(session, attachments + buildId)
            require(items.value.any { it.id == buildId && it.isApk }) { "请重新选择当前电脑的 APK 构建。" }
            require(attachments.all { id -> items.value.any { it.id == id && it.kind == "file" } }) { "反馈附件必须是当前电脑已收到的文件。" }
            verifiedApi(session).json("/api/v2/feedback", "POST", payload)
        }
        withContext(Dispatchers.Main) { requireSession(session); done() }
        message("反馈已关联到此构建。")
        refresh()
    }

    fun setTestStatus(item: FeedItem, status: String) = action {
        val session = store.session.value ?: throw IllegalStateException("请先连接电脑。")
        refreshLock.withLock {
            requireFeed(session, listOf(item.id))
            val updated = FeedItem.parse(verifiedApi(session).json("/api/v2/items/${LinkPolicy.id(item.id)}", "PATCH", JSONObject().put("test_status", status)))
            requireSession(session)
            items.value = items.value.map { if (it.id == updated.id) updated else it }
        }
        message("测试状态已按你的确认记录。")
        refresh()
    }

    fun enqueueFiles(uris: List<Uri>, project: String, replaceId: String?, started: () -> Unit) = action {
        require(uris.size <= 100) { "单次最多添加 100 个文件。" }
        refreshLock.withLock {
            if (replaceId != null) {
                require(uris.size == 1)
                store.selectFile(uris.single(), project, replaceId = replaceId)
            } else store.addAll(uris.map { store.prepareFile(it, project) })
        }
        withContext(Dispatchers.Main) { started() }
    }
    fun enqueueDownload(item: FeedItem, started: () -> Unit) = action {
        val session = store.session.value ?: throw IllegalStateException("请先连接电脑。")
        val export = item.kind == "feedback"
        refreshLock.withLock {
            requireFeed(session, listOf(item.id))
            if (store.tasks.value.any { it.itemId == item.id && it.serverId == session.serverId && it.deviceId == session.deviceId && !it.terminal }) {
                message("这个文件已在传输列表中，可查看进度或继续传输。")
                return@withLock
            }
            store.add(TransferTask(direction = if (export) "export" else "download", origin = session.origin, serverId = session.serverId,
                deviceId = session.deviceId, name = if (export) "feedback-${item.id.take(16)}.md" else item.name,
                mime = if (export) "text/markdown" else item.mime, size = if (export) -1 else item.size,
                sha256 = if (export) "" else item.sha256, itemId = item.id, project = item.project))
        }
        withContext(Dispatchers.Main) { started() }
    }
    fun resume(task: TransferTask, started: () -> Unit) = action {
        val session = store.session.value ?: throw IllegalStateException("请先连接原电脑。")
        TransferRules.validateIdentity(task, session)
        var resumed = false
        store.update(task.id) { current ->
            TransferRules.resume(current, session).also { resumed = it.status == "queued" && current.status != "queued" }
        }
        if (resumed) withContext(Dispatchers.Main) { started() }
    }
    fun pause(task: TransferTask) = action { store.update(task.id) { if (it.terminal) it else it.copy(status = "paused", error = "已暂停，已确认的进度保留。") } }
    fun cancel(task: TransferTask) = action {
        store.update(task.id) { if (it.status == "complete") it else TransferRules.markCancelled(it) }
        val current = store.task(task.id) ?: return@action
        if (current.cancelPending) {
            runCatching {
                refreshLock.withLock {
                    val session = store.session.value ?: throw IllegalStateException("请连接原电脑后重试取消。")
                    TransferCancellation.synchronize(store, current, session)
                }
            }.onFailure {
                store.update(task.id) { latest -> if (latest.cancelPending) latest.copy(error = "本机已取消，电脑端取消尚未确认。连接原电脑后可重试取消。") else latest }
            }
        }
    }

    fun action(block: suspend () -> Unit) {
        viewModelScope.launch {
            busy.value = true
            try { withContext(Dispatchers.IO) { block() } }
            catch (error: Exception) { message(error.message?.take(300) ?: "操作失败，请重试。") }
            finally { busy.value = false }
        }
    }
}
