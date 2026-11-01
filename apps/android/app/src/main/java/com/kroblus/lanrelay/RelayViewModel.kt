package com.kroblus.lanrelay

import android.app.Application
import android.net.Uri
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
    val items = MutableStateFlow<List<FeedItem>>(emptyList())
    val busy = MutableStateFlow(false)
    val online = MutableStateFlow(false)
    val moreAvailable = MutableStateFlow(false)
    val incomingAddress = MutableStateFlow("")
    val sharedFiles = MutableStateFlow<List<Uri>>(emptyList())
    val sharedText = MutableStateFlow("")
    private val visible = MutableStateFlow(false)
    private val refreshLock = Mutex()
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
    fun message(value: String) { store.notice.value = value }

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
        if (expectedPair != null && store.pending.value?.id != expectedPair) return@withLock false
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
        if (link.inviteToken.isEmpty()) require(code.matches(Regex("[0-9]{6}"))) { "请输入电脑当前显示的六位连接码。" }
        val payload = JSONObject().put("name", name.trim())
        if (link.inviteToken.isNotEmpty()) payload.put("token", link.inviteToken) else payload.put("code", code)
        val result = api.json("/api/v2/pair/requests", "POST", payload, authenticated = false)
        store.setPending(PendingPair(link.origin, hello.serverId, hello.name, result.getString("id"), result.getString("claim_token"), result.getLong("expires_at")))
        store.preferences.edit().putString("last_origin", link.origin).putString("device_name", name.trim()).apply()
        message("配对请求已发送，请在电脑窗口批准。")
    }

    private suspend fun pollPair(pending: PendingPair) {
        if (pending.expiresAt < System.currentTimeMillis()) { store.setPending(null); message("配对已过期，请重新扫描电脑二维码。"); return }
        runCatching {
            withContext(Dispatchers.IO) {
                val api = RelayApi(pending.origin)
                api.verifyServer(pending.serverId)
                val result = api.json("/api/v2/pair/claim", "POST", JSONObject().put("id", pending.id).put("claim_token", pending.claimToken), authenticated = false)
                if (store.pending.value?.id != pending.id) return@withContext
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

    fun cancelPairing() = action { store.setPending(null); message("已停止等待，电脑上的请求会自动过期。") }
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
        refreshLock.withLock { uris.forEach { store.selectFile(it, project, replaceId = replaceId) } }
        withContext(Dispatchers.Main) { started() }
    }
    fun enqueueDownload(item: FeedItem, started: () -> Unit) = action {
        val session = store.session.value ?: throw IllegalStateException("请先连接电脑。")
        val export = item.kind == "feedback"
        refreshLock.withLock {
            requireFeed(session, listOf(item.id))
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
        store.update(task.id) { it.copy(status = "queued", error = "") }
        withContext(Dispatchers.Main) { started() }
    }
    fun pause(task: TransferTask) = action { store.update(task.id) { it.copy(status = "paused", error = "已暂停，已确认的进度保留。") } }
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
