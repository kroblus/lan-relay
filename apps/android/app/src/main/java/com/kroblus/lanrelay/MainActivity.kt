package com.kroblus.lanrelay

import android.Manifest
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.viewModels
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.content.ContextCompat
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import java.util.UUID

class MainActivity : ComponentActivity() {
    private val model: RelayViewModel by viewModels()
    private var permissionAction: (() -> Unit)? = null
    private var replacementId: String? = null
    private var selectedProject = ""
    private val permissionRequest = registerForActivityResult(ActivityResultContracts.RequestMultiplePermissions()) {
        val action = permissionAction
        permissionAction = null
        if (localPermissionGranted()) action?.invoke() else model.message("局域网权限未授予。你可以在设置中允许附近设备访问后重试。")
    }
    private val filePicker = registerForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { uris ->
        val replace = replacementId
        replacementId = null
        if (uris.isNotEmpty()) {
            if (replace != null && uris.size != 1) model.message("续传时请只选择一个原文件。")
            else model.enqueueFiles(uris, selectedProject, replace, ::startTransfers)
        }
    }
    private val scanner = registerForActivityResult(ScanContract()) { result ->
        result.contents?.let { model.incomingAddress.value = it }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (savedInstanceState == null) receiveIntent(intent)
        setContent {
            MaterialTheme(colorScheme = lightColorScheme(primary = Color(0xFFFF6A00), onPrimary = Color.White,
                primaryContainer = Color(0xFFFFE5D1), onPrimaryContainer = Color(0xFF512400),
                background = Color(0xFFFAF8F4), surface = Color(0xFFFFFDF9), onSurface = Color(0xFF242424),
                secondaryContainer = Color(0xFFF0ECE5), onSecondaryContainer = Color(0xFF4B4841))) {
                RelayScreen(model,
                    onConnect = { address, name, code -> permissions(false) { model.connect(address, name, code) } },
                    onScan = { scanner.launch(ScanOptions().setDesiredBarcodeFormats(ScanOptions.QR_CODE).setPrompt("扫描电脑窗口中的配对二维码").setBeepEnabled(false).setOrientationLocked(false)) },
                    onPick = { project, replacement -> selectedProject = project; replacementId = replacement; filePicker.launch(arrayOf("*/*")) },
                    onStartTransfers = ::startTransfers,
                    onCopy = ::copy,
                    onOpen = ::openFile)
            }
        }
    }
    override fun onStart() { super.onStart(); model.visible(true) }
    override fun onStop() { model.visible(false); super.onStop() }
    override fun onNewIntent(intent: Intent) { super.onNewIntent(intent); setIntent(intent); receiveIntent(intent) }

    private fun localPermissionGranted() = Build.VERSION.SDK_INT < 37 || ContextCompat.checkSelfPermission(this, "android.permission.ACCESS_LOCAL_NETWORK") == PackageManager.PERMISSION_GRANTED
    private fun permissions(notifications: Boolean, action: () -> Unit) {
        val wanted = mutableListOf<String>()
        if (!localPermissionGranted()) wanted += "android.permission.ACCESS_LOCAL_NETWORK"
        if (notifications && Build.VERSION.SDK_INT >= 33 && ContextCompat.checkSelfPermission(this, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED &&
            !model.store.preferences.getBoolean("notifications_requested", false)) {
            wanted += Manifest.permission.POST_NOTIFICATIONS
            model.store.preferences.edit().putBoolean("notifications_requested", true).apply()
        }
        if (wanted.isEmpty()) action() else { permissionAction = action; permissionRequest.launch(wanted.toTypedArray()) }
    }
    private fun startTransfers() = permissions(true) {
        try { ContextCompat.startForegroundService(this, Intent(this, TransferService::class.java)) }
        catch (error: Exception) {
            model.store.pauseAll("系统暂不允许后台传输，请回到应用后继续。")
            model.message(error.message?.take(200) ?: "无法启动传输，请保持应用在前台后重试。")
        }
    }
    private fun copy(text: String) {
        getSystemService(ClipboardManager::class.java).setPrimaryClip(ClipData.newPlainText("随手传", text))
        model.message("已复制。")
    }
    private fun openFile(task: TransferTask) {
        try {
            val view = Intent(Intent.ACTION_VIEW).setDataAndType(Uri.parse(task.savedUri), task.mime)
                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            startActivity(Intent.createChooser(view, "打开已保存的文件"))
        } catch (_: Exception) { model.message("没有可打开此文件的应用，请到下载/LANRelay 目录查看。") }
    }
    @Suppress("DEPRECATION")
    private fun receiveIntent(incoming: Intent?) {
        if (incoming == null) return
        when (incoming.action) {
            Intent.ACTION_VIEW -> incoming.dataString?.let { model.incomingAddress.value = it }
            Intent.ACTION_SEND, Intent.ACTION_SEND_MULTIPLE -> {
                val uris = linkedSetOf<Uri>()
                if (incoming.action == Intent.ACTION_SEND) {
                    val uri = if (Build.VERSION.SDK_INT >= 33) incoming.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java) else incoming.getParcelableExtra<Uri>(Intent.EXTRA_STREAM)
                    if (uri != null) uris += uri
                } else {
                    val extra = if (Build.VERSION.SDK_INT >= 33) incoming.getParcelableArrayListExtra(Intent.EXTRA_STREAM, Uri::class.java) else incoming.getParcelableArrayListExtra<Uri>(Intent.EXTRA_STREAM)
                    uris += extra.orEmpty()
                }
                incoming.clipData?.let { clip -> for (i in 0 until minOf(clip.itemCount, 100)) clip.getItemAt(i).uri?.let { uris += it } }
                model.sharedFiles.value = uris.filter { it.scheme == "content" }.take(100)
                model.sharedText.value = incoming.getCharSequenceExtra(Intent.EXTRA_TEXT)?.toString().orEmpty().take(65536)
                model.message("已接收分享内容，请确认目标电脑后发送。")
            }
        }
    }
}

@Composable
private fun RelayScreen(model: RelayViewModel, onConnect: (String, String, String) -> Unit, onScan: () -> Unit,
                        onPick: (String, String?) -> Unit, onStartTransfers: () -> Unit, onCopy: (String) -> Unit, onOpen: (TransferTask) -> Unit) {
    val session by model.store.session.collectAsStateWithLifecycle()
    val connections by model.store.connections.collectAsStateWithLifecycle()
    val pending by model.store.pending.collectAsStateWithLifecycle()
    val tasks by model.store.tasks.collectAsStateWithLifecycle()
    val feed by model.items.collectAsStateWithLifecycle()
    val online by model.online.collectAsStateWithLifecycle()
    val busy by model.busy.collectAsStateWithLifecycle()
    val notice by model.store.notice.collectAsStateWithLifecycle()
    val more by model.moreAvailable.collectAsStateWithLifecycle()
    val incoming by model.incomingAddress.collectAsStateWithLifecycle()
    val sharedFiles by model.sharedFiles.collectAsStateWithLifecycle()
    val sharedText by model.sharedText.collectAsStateWithLifecycle()
    var tab by rememberSaveable { mutableIntStateOf(if (session == null) 2 else 0) }
    var project by rememberSaveable { mutableStateOf(model.store.preferences.getString("project", "").orEmpty()) }
    var text by rememberSaveable { mutableStateOf("") }
    var address by rememberSaveable { mutableStateOf(model.store.preferences.getString("last_origin", "").orEmpty()) }
    var name by rememberSaveable { mutableStateOf(model.store.preferences.getString("device_name", "我的手机").orEmpty()) }
    var code by rememberSaveable { mutableStateOf("") }
    var invitePayload by remember { mutableStateOf("") }
    var feedbackBuild by remember { mutableStateOf<FeedItem?>(null) }
    var statusItem by remember { mutableStateOf<FeedItem?>(null) }
    var disconnectDialog by remember { mutableStateOf(false) }
    var showBuildsOnly by rememberSaveable { mutableStateOf(false) }
    var filterProject by rememberSaveable { mutableStateOf(false) }
    val visibleFeed = feed.filter { (!showBuildsOnly || it.isApk || it.kind == "feedback") && (!filterProject || project.isBlank() || it.project == project) }
    val textRequestId = remember(text, project) { UUID.randomUUID().toString() }
    LaunchedEffect(sharedText) { if (sharedText.isNotEmpty()) { text = sharedText; tab = 0; model.sharedText.value = "" } }
    LaunchedEffect(incoming) {
        if (incoming.isNotEmpty()) {
            runCatching { LinkPolicy.parse(incoming) }.onSuccess {
                address = it.origin
                invitePayload = if (it.inviteToken.isNotEmpty()) incoming else ""
                code = ""
                tab = 2
            }.onFailure { model.message(it.message ?: "二维码无法识别。") }
            model.incomingAddress.value = ""
        }
    }
    LaunchedEffect(session?.serverId, session?.deviceId) {
        feedbackBuild = null
        statusItem = null
        if (session != null) { tab = 0; model.refreshNow() }
    }
    LaunchedEffect(project) { model.store.preferences.edit().putString("project", project).apply() }

    Surface(Modifier.fillMaxSize().safeDrawingPadding().imePadding(), color = MaterialTheme.colorScheme.background) {
        Column(Modifier.fillMaxSize()) {
            Column(Modifier.padding(horizontal = 20.dp, vertical = 14.dp)) {
                Text("随手传", fontSize = 28.sp, fontWeight = FontWeight.Bold)
                Text(when { pending != null -> "等待电脑批准"; session == null -> "手机与电脑，轻松互传"; online -> "已连接 · ${session!!.serverName}"; else -> "等待电脑连接 · ${session!!.serverName}" },
                    color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 2)
            }
            TabRow(selectedTabIndex = tab) {
                listOf("内容", "传输", "连接").forEachIndexed { index, title ->
                    Tab(selected = tab == index, onClick = { tab = index }, text = { Text(if (index == 1 && tasks.any { !it.terminal }) "$title ${tasks.count { !it.terminal }}" else title) })
                }
            }
            if (notice.isNotEmpty()) Surface(color = MaterialTheme.colorScheme.secondaryContainer) {
                Row(Modifier.fillMaxWidth().padding(start = 16.dp, top = 4.dp, bottom = 4.dp)) {
                    Text(notice, Modifier.weight(1f).padding(vertical = 9.dp), style = MaterialTheme.typography.bodySmall)
                    TextButton(onClick = { model.message("") }) { Text("收起") }
                }
            }
            when (tab) {
                0 -> LazyColumn(Modifier.weight(1f).fillMaxWidth(), contentPadding = PaddingValues(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    if (sharedFiles.isNotEmpty()) item {
                        Card {
                            Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                                Text("收到 ${sharedFiles.size} 个分享文件", fontWeight = FontWeight.SemiBold)
                                Text("目标：${session?.serverName ?: "请先连接电脑"}")
                                Row {
                                    Button(enabled = session != null && !busy, onClick = {
                                        model.enqueueFiles(sharedFiles, project, null) { model.sharedFiles.value = emptyList(); tab = 1; onStartTransfers() }
                                    }) { Text("确认发送") }
                                    TextButton(onClick = { model.sharedFiles.value = emptyList() }) { Text("取消") }
                                }
                            }
                        }
                    }
                    item {
                        Card(shape = RoundedCornerShape(20.dp)) {
                            Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                                OutlinedTextField(value = project, onValueChange = { project = it.take(120) }, modifier = Modifier.fillMaxWidth(), singleLine = true,
                                    label = { Text("项目（可选）") }, placeholder = { Text("记住你上次使用的项目") })
                                OutlinedTextField(value = text, onValueChange = { text = it.take(65536) }, modifier = Modifier.fillMaxWidth(), minLines = 2, maxLines = 6,
                                    label = { Text("文字、链接或日志") }, placeholder = { Text("在这里粘贴，原样发送") })
                                Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                                    OutlinedButton(modifier = Modifier.weight(1f), enabled = session != null && !busy, onClick = { onPick(project, null) }) { Text("选择文件") }
                                    Button(modifier = Modifier.weight(1f), enabled = session != null && text.isNotBlank() && !busy,
                                        onClick = { model.sendText(text, project, textRequestId) { text = "" } }) { Text("发送文字") }
                                }
                            }
                        }
                    }
                    item {
                        Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
                            Text("最近内容", style = MaterialTheme.typography.titleMedium, modifier = Modifier.padding(top = 12.dp))
                            TextButton(onClick = model::refreshNow, enabled = session != null) { Text("刷新") }
                        }
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            FilterChip(selected = showBuildsOnly, onClick = { showBuildsOnly = !showBuildsOnly }, label = { Text("构建与反馈") })
                            if (project.isNotBlank()) FilterChip(selected = filterProject, onClick = { filterProject = !filterProject }, label = { Text("当前项目") })
                        }
                    }
                    if (session == null) item { EmptyCard("先连接电脑", "两台设备连接到可互访的局域网，然后扫描电脑窗口中的二维码。") }
                    else if (feed.isEmpty()) item { EmptyCard("从第一条内容开始", "电脑发送的文件和文字会出现在这里。") }
                    items(visibleFeed, key = { it.id }) { item ->
                        FeedCard(item, busy, onCopy, onDownload = { model.enqueueDownload(item) { tab = 1; onStartTransfers() } },
                            onFeedback = { feedbackBuild = item }, onStatus = { statusItem = item })
                    }
                    if (more) item { OutlinedButton(modifier = Modifier.fillMaxWidth(), enabled = !busy, onClick = model::loadMore) { Text("加载更早内容") } }
                }
                1 -> LazyColumn(Modifier.weight(1f).fillMaxWidth(), contentPadding = PaddingValues(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                    item {
                        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            OutlinedButton(enabled = tasks.any { it.active || it.status == "queued" }, onClick = { model.action { model.store.pauseAll() } }) { Text("暂停全部") }
                            TextButton(onClick = { model.action { model.store.clearFinished() } }, enabled = tasks.any { it.clearable }) { Text("清理完成记录") }
                        }
                    }
                    if (tasks.isEmpty()) item { EmptyCard("传输记录会保留在这里", "暂停后可继续；文件授权失效时可以重新选择原文件。") }
                    items(tasks.reversed(), key = { it.id }) { task ->
                        TransferCard(task, busy, onResume = { model.resume(task, onStartTransfers) }, onPause = { model.pause(task) },
                            onCancel = { model.cancel(task) }, onReselect = { onPick(task.project, task.id) }, onOpen = { onOpen(task) })
                    }
                }
                2 -> LazyColumn(Modifier.weight(1f).fillMaxWidth(), contentPadding = PaddingValues(20.dp), verticalArrangement = Arrangement.spacedBy(16.dp)) {
                    item { Text("连接你的电脑", style = MaterialTheme.typography.headlineSmall, fontWeight = FontWeight.SemiBold) }
                    item { Text("在电脑打开新版随手传。扫码或输入它显示的局域网地址，再由电脑批准连接。", color = MaterialTheme.colorScheme.onSurfaceVariant) }
                    if (connections.isNotEmpty()) item {
                        Column {
                            Text("最近连接", style = MaterialTheme.typography.labelLarge)
                            connections.forEach { known -> TextButton(enabled = !busy && pending == null, onClick = {
                                address = known.origin; invitePayload = ""; code = ""; onConnect(known.origin, name, "")
                            }) { Text("${known.serverName} · ${known.origin}", maxLines = 2) } }
                        }
                    }
                    item { Button(onClick = onScan, modifier = Modifier.fillMaxWidth(), enabled = !busy) { Text("扫描电脑二维码") } }
                    item { OutlinedTextField(value = address, onValueChange = { address = it; invitePayload = "" }, label = { Text("电脑地址") }, placeholder = { Text("192.168.1.10:4318") }, singleLine = true, modifier = Modifier.fillMaxWidth(), keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri)) }
                    item {
                        OutlinedTextField(value = name, onValueChange = { name = it.take(80) }, label = { Text("设备名称") }, singleLine = true, modifier = Modifier.fillMaxWidth())
                        Row { listOf("我的手机", "我的平板", "另一台电脑").forEach { preset -> TextButton(onClick = { name = preset }) { Text(preset) } } }
                    }
                    if (invitePayload.isEmpty()) item { OutlinedTextField(value = code, onValueChange = { code = it.filter(Char::isDigit).take(6) }, label = { Text("六位连接码（已配对可留空）") }, singleLine = true, modifier = Modifier.fillMaxWidth(), keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.NumberPassword)) }
                    else item { Text("已读取本次扫码邀请，确认地址后连接。", color = MaterialTheme.colorScheme.primary) }
                    item { Text("仅在可信局域网使用。HTTP 传输没有加密；连接前请核对电脑地址。", style = MaterialTheme.typography.bodySmall) }
                    item { Button(enabled = !busy && pending == null && address.isNotBlank(), onClick = { onConnect(invitePayload.ifEmpty { address }, name, code) }, modifier = Modifier.fillMaxWidth()) { Text(if (busy) "正在连接…" else "确认地址并请求批准") } }
                    if (pending != null) item { OutlinedButton(onClick = model::cancelPairing, modifier = Modifier.fillMaxWidth()) { Text("停止等待电脑批准") } }
                    if (session != null) {
                        item { HorizontalDivider() }
                        item { Text("当前电脑：${session!!.serverName}\n${session!!.origin}", style = MaterialTheme.typography.bodyMedium) }
                        item { TextButton(onClick = { disconnectDialog = true }) { Text("断开当前连接") } }
                    }
                    item { Text("手机 v${BuildConfig.VERSION_NAME} · 协议 2\n电脑需保持运行。下载成功不代表 APK 已安装或测试通过。", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
                }
            }
        }
    }
    feedbackBuild?.let { build ->
        FeedbackDialog(build, feed.filter { it.kind == "file" && it.id != build.id }, busy, onDismiss = { feedbackBuild = null },
            onSend = { body, attachments, id -> model.sendFeedback(build.id, body, attachments, id) { feedbackBuild = null } })
    }
    statusItem?.let { item ->
        AlertDialog(onDismissRequest = { statusItem = null }, title = { Text("记录实际测试状态") },
            text = { Column { Text("仅记录你亲自确认的结果：${item.name}"); listOf("unverified", "installed", "passed", "failed", "blocked").forEach { status -> TextButton(onClick = { statusItem = null; model.setTestStatus(item, status) }) { Text(testStatusLabel(status)) } } } },
            confirmButton = { TextButton(onClick = { statusItem = null }) { Text("取消") } })
    }
    if (disconnectDialog) AlertDialog(onDismissRequest = { disconnectDialog = false }, title = { Text("断开当前连接？") },
        text = { Text("传输会暂停，任务和已保存文件保留。可以通过最近连接恢复原设备身份；电脑撤销后需要重新配对。") },
        confirmButton = { TextButton(onClick = { disconnectDialog = false; model.disconnect() }) { Text("断开") } },
        dismissButton = { TextButton(onClick = { disconnectDialog = false }) { Text("保持连接") } })
}

@Composable
private fun EmptyCard(title: String, body: String) {
    Card(Modifier.fillMaxWidth()) { Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) { Text(title, fontWeight = FontWeight.SemiBold); Text(body, color = MaterialTheme.colorScheme.onSurfaceVariant) } }
}

@Composable
private fun FeedCard(item: FeedItem, busy: Boolean, onCopy: (String) -> Unit, onDownload: () -> Unit, onFeedback: () -> Unit, onStatus: () -> Unit) {
    Card(Modifier.fillMaxWidth(), shape = RoundedCornerShape(18.dp)) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(when { item.kind == "feedback" -> "测试反馈"; item.kind == "text" -> "文字"; item.isApk -> "APK · ${item.name}"; else -> item.name }, fontWeight = FontWeight.SemiBold, maxLines = 3, overflow = TextOverflow.Ellipsis)
            Text(listOf(item.project, SimpleDateFormat("MM-dd HH:mm", Locale.CHINA).format(Date(item.createdAt))).filter { it.isNotBlank() }.joinToString(" · "), style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
            if (item.text.isNotEmpty()) {
                var expanded by remember(item.id) { mutableStateOf(false) }
                SelectionContainer { Text(item.text, maxLines = if (expanded) Int.MAX_VALUE else 7, overflow = TextOverflow.Ellipsis) }
                if (item.text.length > 240 || item.text.count { it == '\n' } > 6) TextButton(onClick = { expanded = !expanded }) { Text(if (expanded) "收起" else "展开文字") }
            }
            if (item.kind == "file") {
                Text("${formatBytes(item.size)} · SHA-256 ${item.sha256.take(12)}…", style = MaterialTheme.typography.bodySmall)
                if (item.apkPackage.isNotEmpty()) Text("${item.apkPackage} ${item.apkVersion}", style = MaterialTheme.typography.bodySmall)
                if (item.isApk) Text(testStatusLabel(item.testStatus), color = MaterialTheme.colorScheme.primary, style = MaterialTheme.typography.labelLarge)
            }
            if (item.kind == "feedback") Text("关联构建 ${item.buildId.take(12)} · ${item.attachments.size} 个附件", style = MaterialTheme.typography.bodySmall)
            Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                if (item.kind == "file" || item.kind == "feedback") Button(onClick = onDownload, enabled = !busy) { Text(if (item.kind == "feedback") "导出反馈" else "保存到手机") }
                if (item.text.isNotEmpty()) TextButton(onClick = { onCopy(item.text) }) { Text("复制") }
                if (item.isApk) TextButton(onClick = onFeedback, enabled = !busy) { Text("写反馈") }
            }
            if (item.isApk) TextButton(onClick = onStatus, enabled = !busy) { Text("记录安装 / 测试状态") }
        }
    }
}

@Composable
private fun TransferCard(task: TransferTask, busy: Boolean, onResume: () -> Unit, onPause: () -> Unit, onCancel: () -> Unit, onReselect: () -> Unit, onOpen: () -> Unit) {
    Card(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text(task.name, fontWeight = FontWeight.SemiBold, maxLines = 2, overflow = TextOverflow.Ellipsis)
            Text("${if (task.direction == "upload") "发送到电脑" else "接收到手机"} · ${task.statusLabel()}", style = MaterialTheme.typography.bodyMedium)
            if (task.size > 0 && !task.terminal) LinearProgressIndicator(progress = { (task.offset.toFloat() / task.size.toFloat()).coerceIn(0f, 1f) }, modifier = Modifier.fillMaxWidth())
            Text("${formatBytes(task.offset)} / ${formatBytes(task.size)}", style = MaterialTheme.typography.bodySmall)
            if (task.error.isNotEmpty()) Text(task.error, style = MaterialTheme.typography.bodySmall, color = if (task.status == "failed") MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant)
            if (!task.terminal) {
                Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    if (task.active || task.status == "queued") TextButton(onClick = onPause, enabled = !busy) { Text("暂停") }
                    else if (task.status != "needs_file") Button(onClick = onResume, enabled = !busy) { Text("继续") }
                    if (task.status == "needs_file") Button(onClick = onReselect, enabled = !busy) { Text("重选原文件") }
                    TextButton(onClick = onCancel, enabled = !busy) { Text("取消") }
                }
            } else if (task.status == "complete" && task.savedUri.isNotEmpty()) TextButton(onClick = onOpen) { Text("打开已保存文件") }
            else if (task.status == "cancelled" && task.cancelPending) TextButton(onClick = onCancel, enabled = !busy) { Text("重试电脑端取消") }
        }
    }
}

@Composable
private fun FeedbackDialog(build: FeedItem, files: List<FeedItem>, busy: Boolean, onDismiss: () -> Unit, onSend: (String, Set<String>, String) -> Unit) {
    var body by rememberSaveable(build.id) { mutableStateOf("") }
    var selected by remember(build.id) { mutableStateOf(emptySet<String>()) }
    val requestId = remember(build.id, body, selected) { UUID.randomUUID().toString() }
    AlertDialog(onDismissRequest = onDismiss, title = { Text("反馈 · ${build.name}", maxLines = 2) },
        text = {
            LazyColumn(Modifier.heightIn(max = 480.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                item { OutlinedTextField(value = body, onValueChange = { body = it.take(32768) }, minLines = 4, maxLines = 10,
                    placeholder = { Text("如何复现？\n预期是什么？\n实际发生了什么？\n真机和系统版本（确认后填写）") }, modifier = Modifier.fillMaxWidth()) }
                item { Text("关联已上传的截图、录屏或日志", style = MaterialTheme.typography.labelLarge) }
                if (files.isEmpty()) item { Text("可以先发送附件，再回到这个构建写反馈。") }
                items(files.take(50), key = { it.id }) { file ->
                    Row(Modifier.fillMaxWidth()) {
                        Checkbox(checked = file.id in selected, onCheckedChange = { checked -> selected = if (checked) selected + file.id else selected - file.id })
                        Text(file.name, Modifier.weight(1f).padding(top = 12.dp), maxLines = 2, overflow = TextOverflow.Ellipsis)
                    }
                }
            }
        }, confirmButton = { Button(enabled = body.isNotBlank() && !busy, onClick = { onSend(body, selected, requestId) }) { Text("提交反馈") } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("取消") } })
}

private fun testStatusLabel(status: String) = when (status) {
    "installed" -> "已人工确认安装"
    "passed" -> "PASS · 测试通过"
    "failed" -> "FAIL · 测试失败"
    "blocked" -> "BLOCKED · 验收受阻"
    else -> "NOT RUN · 尚未验证"
}
