package com.kroblus.lanrelay

import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import java.util.UUID

@Composable
internal fun RelayScreen(
    model: RelayViewModel,
    onConnect: (String, String, String) -> Unit,
    onScan: () -> Unit,
    onPick: (String, String?) -> Unit,
    onStartTransfers: () -> Unit,
    onCopy: (String) -> Unit,
    onOpen: (TransferTask) -> Unit
) {
    val session by model.store.session.collectAsStateWithLifecycle()
    val connections by model.store.connections.collectAsStateWithLifecycle()
    val pending by model.store.pending.collectAsStateWithLifecycle()
    val tasks by model.store.tasks.collectAsStateWithLifecycle()
    val metrics by model.store.metrics.collectAsStateWithLifecycle()
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
    var composer by rememberSaveable { mutableStateOf(false) }
    var address by rememberSaveable { mutableStateOf(model.store.preferences.getString("last_origin", "").orEmpty()) }
    var name by rememberSaveable { mutableStateOf(model.store.preferences.getString("device_name", "我的手机").orEmpty()) }
    var code by rememberSaveable { mutableStateOf("") }
    var invitePayload by remember { mutableStateOf("") }
    var manual by rememberSaveable { mutableStateOf(false) }
    var projectDialog by remember { mutableStateOf(false) }
    var nameDialog by remember { mutableStateOf(false) }
    var feedbackBuild by remember { mutableStateOf<FeedItem?>(null) }
    var statusItem by remember { mutableStateOf<FeedItem?>(null) }
    var disconnectDialog by remember { mutableStateOf(false) }
    var filter by rememberSaveable { mutableIntStateOf(0) }
    var filterProject by rememberSaveable { mutableStateOf(false) }
    val visibleFeed = feed.filter {
        (when (filter) { 1 -> it.kind == "file"; 2 -> it.kind == "text"; 3 -> it.isApk || it.kind == "feedback"; else -> true }) &&
            (!filterProject || project.isBlank() || it.project == project)
    }
    val textRequestId = remember(text, project) { UUID.randomUUID().toString() }
    LaunchedEffect(sharedText) {
        if (sharedText.isNotEmpty()) { text = sharedText; composer = true; tab = 0; model.sharedText.value = "" }
    }
    LaunchedEffect(sharedFiles) { if (sharedFiles.isNotEmpty()) tab = 0 }
    LaunchedEffect(incoming) {
        if (incoming.isNotEmpty()) {
            runCatching { LinkPolicy.parse(incoming) }.onSuccess {
                address = it.origin
                invitePayload = if (it.inviteToken.isNotEmpty()) incoming else ""
                manual = it.inviteToken.isEmpty()
                code = ""
                tab = 2
            }.onFailure { model.message(it.message ?: "二维码无法识别。") }
            model.incomingAddress.value = ""
        }
    }
    LaunchedEffect(session?.serverId, session?.deviceId) {
        feedbackBuild = null
        statusItem = null
        if (session != null) { tab = 0; invitePayload = ""; model.refreshNow() }
    }
    LaunchedEffect(project) { model.store.preferences.edit().putString("project", project).apply() }
    LaunchedEffect(name) { model.store.preferences.edit().putString("device_name", name).apply() }
    Surface(Modifier.fillMaxSize().safeDrawingPadding().imePadding(), color = RelayColors.Background) {
        Column(Modifier.fillMaxSize()) {
            RelayHeader(session, online, pending != null, onDevices = { tab = 2 })
            if (notice.isNotEmpty()) NoticeCard(notice, onDismiss = { model.message("") })
            when (tab) {
                0 -> LazyColumn(
                    Modifier.weight(1f).fillMaxWidth(),
                    contentPadding = PaddingValues(start = 20.dp, end = 20.dp, top = 8.dp, bottom = 24.dp),
                    verticalArrangement = Arrangement.spacedBy(16.dp)
                ) {
                    item {
                        SendHero(
                            connected = session != null, busy = busy, project = project,
                            onFiles = { if (session == null) tab = 2 else onPick(project, null) },
                            onText = { composer = !composer }, onProject = { projectDialog = true }
                        )
                    }
                    if (sharedFiles.isNotEmpty()) item {
                        RelayCard(tone = RelayColors.Apricot) {
                            Text("${sharedFiles.size} 个文件，准备发送", style = MaterialTheme.typography.titleMedium)
                            Text("发送到 ${session?.serverName ?: "电脑 · 请先连接"}", color = RelayColors.Muted)
                            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                Button(enabled = session != null && !busy, onClick = {
                                    model.enqueueFiles(sharedFiles, project, null) { model.sharedFiles.value = emptyList(); tab = 1; onStartTransfers() }
                                }) { Text("确认发送") }
                                TextButton(onClick = { model.sharedFiles.value = emptyList() }) { Text("取消") }
                            }
                        }
                    }
                    if (composer) item {
                        TextComposer(text, busy, session != null, onText = { text = it.take(65536) },
                            onClose = { composer = false },
                            onSend = { model.sendText(text, project, textRequestId) { text = ""; composer = false } })
                    }
                    item {
                        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                            Text("最近内容", style = MaterialTheme.typography.titleLarge, modifier = Modifier.weight(1f))
                            IconButton(onClick = model::refreshNow, enabled = session != null,
                                modifier = Modifier.semantics { contentDescription = "刷新最近内容" }) { RelayIcon(RelayGlyph.Refresh, tint = RelayColors.Muted) }
                        }
                        LazyRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            items(4) { index ->
                                FilterChip(selected = filter == index, onClick = { filter = index },
                                    label = { Text(listOf("全部", "文件", "文字", "构建与反馈")[index]) },
                                    border = if (filter == index) null else BorderStroke(1.dp, RelayColors.Line),
                                    colors = FilterChipDefaults.filterChipColors(selectedContainerColor = RelayColors.SageLight, selectedLabelColor = RelayColors.Sage))
                            }
                            if (project.isNotBlank()) item {
                                FilterChip(selected = filterProject, onClick = { filterProject = !filterProject }, label = { Text("当前项目") })
                            }
                        }
                    }
                    if (session == null) item {
                        EmptyCard(RelayGlyph.Devices, "连接后，内容就在这里", "让手机与电脑加入同一个局域网，扫描电脑上的二维码即可开始。",
                            action = "连接电脑", onAction = { tab = 2 })
                    } else if (visibleFeed.isEmpty()) item {
                        EmptyCard(RelayGlyph.Folder, if (feed.isEmpty()) "传一份喜欢的内容吧" else "这里还没有内容",
                            if (feed.isEmpty()) "电脑发来的文件、图片和文字，会安静地收在这里。" else "试试其他分类，或刷新最近内容。")
                    }
                    items(visibleFeed, key = { it.id }) { item ->
                        FeedCard(item, busy, onCopy,
                            onDownload = { model.enqueueDownload(item) { tab = 1; onStartTransfers() } },
                            onFeedback = { feedbackBuild = item }, onStatus = { statusItem = item })
                    }
                    if (more) item {
                        TextButton(modifier = Modifier.fillMaxWidth(), enabled = !busy, onClick = model::loadMore) { Text("查看更早内容") }
                    }
                }
                1 -> LazyColumn(
                    Modifier.weight(1f).fillMaxWidth(),
                    contentPadding = PaddingValues(start = 20.dp, end = 20.dp, top = 8.dp, bottom = 24.dp),
                    verticalArrangement = Arrangement.spacedBy(16.dp)
                ) {
                    item {
                        Text("每一份，都有进度", style = MaterialTheme.typography.headlineSmall)
                        Spacer(Modifier.height(8.dp))
                        Text("${tasks.count { !it.terminal || it.cancelPending }} 项待完成 · ${tasks.count { it.status == "complete" }} 项已完成",
                            style = MaterialTheme.typography.bodyMedium, color = RelayColors.Muted)
                        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            TextButton(enabled = tasks.any { it.active || it.status == "queued" },
                                onClick = { model.action { model.store.pauseAll() } }) { RelayIcon(RelayGlyph.Pause, Modifier.size(18.dp)); Spacer(Modifier.width(6.dp)); Text("暂停全部") }
                            TextButton(enabled = tasks.any { it.clearable }, onClick = { model.action { model.store.clearFinished() } }) { Text("清理完成记录") }
                        }
                    }
                    if (tasks.isEmpty()) item {
                        EmptyCard(RelayGlyph.Transfer, "还没有传输任务", "文件的发送和接收进度都会留在这里。暂停后，也能接着传。", action = "去发送文件", onAction = { tab = 0 })
                    }
                    items(tasks.asReversed().sortedBy { it.terminal && !it.cancelPending }, key = { it.id }) { task ->
                        TransferCard(task, metrics[task.id], busy,
                            onResume = { model.resume(task, onStartTransfers) }, onPause = { model.pause(task) },
                            onCancel = { model.cancel(task) }, onReselect = { onPick(task.project, task.id) }, onOpen = { onOpen(task) })
                    }
                }
                2 -> LazyColumn(
                    Modifier.weight(1f).fillMaxWidth(),
                    contentPadding = PaddingValues(start = 20.dp, end = 20.dp, top = 8.dp, bottom = 24.dp),
                    verticalArrangement = Arrangement.spacedBy(16.dp)
                ) {
                    item {
                        Text("让设备，靠近一点", style = MaterialTheme.typography.headlineSmall)
                        Spacer(Modifier.height(8.dp))
                        Text("同一个局域网，手机与电脑轻松互传。", style = MaterialTheme.typography.bodyMedium, color = RelayColors.Muted)
                    }
                    if (session != null) item {
                        RelayCard {
                            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                                IconTile(RelayGlyph.Devices, RelayColors.SageLight, RelayColors.Sage)
                                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                                    Text(session!!.serverName, style = MaterialTheme.typography.titleMedium, maxLines = 2, overflow = TextOverflow.Ellipsis)
                                    Text(if (online) "已连接 · 可以开始传输" else "等待电脑上线 · 任务保留", style = MaterialTheme.typography.bodySmall, color = if (online) RelayColors.Sage else RelayColors.Muted)
                                }
                            }
                            SelectionContainer { Text(session!!.origin, style = MaterialTheme.typography.bodySmall, color = RelayColors.Muted) }
                            FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                                FilledTonalButton(onClick = { tab = 0 }) { Text("去收发内容") }
                                TextButton(onClick = { disconnectDialog = true }) { Text("断开连接") }
                            }
                        }
                    }
                    if (pending != null) item {
                        RelayCard(tone = RelayColors.Lavender) {
                            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                                CircularProgressIndicator(Modifier.size(22.dp), strokeWidth = 2.dp, color = RelayColors.LilacInk)
                                Text("等待电脑点一下批准", style = MaterialTheme.typography.titleMedium)
                            }
                            Text("${pending!!.serverName}\n${pending!!.origin}", style = MaterialTheme.typography.bodyMedium)
                            TextButton(onClick = model::cancelPairing) { Text("停止等待") }
                        }
                    } else item {
                        RelayCard(tone = RelayColors.SageLight) {
                            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(16.dp)) {
                                Surface(shape = RoundedCornerShape(18.dp), color = Color.White) { RelayIcon(RelayGlyph.Scan, Modifier.padding(15.dp).size(38.dp), RelayColors.Sage) }
                                Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                                    Text(if (session == null) "扫一扫，就连上" else "连接另一台电脑", style = MaterialTheme.typography.titleLarge)
                                    Text("打开电脑端「随手传」，找到配对二维码。", style = MaterialTheme.typography.bodyMedium, color = RelayColors.Muted)
                                }
                            }
                            Button(onClick = onScan, modifier = Modifier.fillMaxWidth().heightIn(min = 52.dp), enabled = !busy) {
                                RelayIcon(RelayGlyph.Scan, Modifier.size(20.dp)); Spacer(Modifier.width(8.dp)); Text("扫描电脑二维码")
                            }
                        }
                    }
                    if (invitePayload.isNotEmpty() && pending == null) item {
                        RelayCard {
                            Text("已读取电脑邀请", style = MaterialTheme.typography.titleMedium)
                            SelectionContainer { Text(address, style = MaterialTheme.typography.bodyMedium) }
                            Text("连接码已随本次扫码带入，核对地址后连接。", style = MaterialTheme.typography.bodySmall, color = RelayColors.Muted)
                            Button(enabled = !busy, onClick = { onConnect(invitePayload, name, "") }, modifier = Modifier.fillMaxWidth()) { Text(if (busy) "正在连接…" else "连接这台电脑") }
                        }
                    }
                    item {
                        RelayCard {
                            Row(Modifier.fillMaxWidth().clickable { nameDialog = true }, verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                                IconTile(RelayGlyph.Edit, RelayColors.Apricot, RelayColors.Ink, small = true)
                                Column(Modifier.weight(1f)) {
                                    Text("这台设备", style = MaterialTheme.typography.bodySmall, color = RelayColors.Muted)
                                    Text(name.ifBlank { "我的手机" }, style = MaterialTheme.typography.titleMedium)
                                }
                                RelayIcon(RelayGlyph.Chevron, Modifier.size(18.dp), RelayColors.Muted)
                            }
                            HorizontalDivider(color = RelayColors.Line)
                            TextButton(onClick = { manual = !manual; if (manual) invitePayload = "" }, modifier = Modifier.fillMaxWidth()) {
                                Text(if (manual) "收起手动连接" else "无法扫码？手动连接")
                            }
                            if (manual) {
                                OutlinedTextField(value = address, onValueChange = { address = it; invitePayload = "" }, label = { Text("电脑地址") },
                                    placeholder = { Text("192.168.1.10:4318") }, singleLine = true, modifier = Modifier.fillMaxWidth(),
                                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri))
                                OutlinedTextField(value = code, onValueChange = { code = it.filter { c -> c in '0'..'9' }.take(6) },
                                    label = { Text("六位连接码") }, supportingText = { Text("已配对的电脑可留空") },
                                    singleLine = true, modifier = Modifier.fillMaxWidth(),
                                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.NumberPassword))
                                Button(enabled = !busy && pending == null && address.isNotBlank() && name.isNotBlank(),
                                    onClick = { onConnect(address, name, code) }, modifier = Modifier.fillMaxWidth()) {
                                    Text(if (busy) "正在连接…" else "请求电脑批准")
                                }
                            }
                        }
                    }
                    if (connections.isNotEmpty()) {
                        item { Text("最近连接", style = MaterialTheme.typography.titleMedium) }
                        items(connections, key = { it.serverId + it.deviceId }) { known ->
                            RelayCard {
                                Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                                    IconTile(RelayGlyph.Devices, RelayColors.Lavender, RelayColors.LilacInk, small = true)
                                    Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                                        Text(known.serverName, style = MaterialTheme.typography.titleSmall, maxLines = 2, overflow = TextOverflow.Ellipsis)
                                        Text(known.origin, style = MaterialTheme.typography.bodySmall, color = RelayColors.Muted)
                                    }
                                }
                                TextButton(enabled = !busy && pending == null, onClick = {
                                    address = known.origin; invitePayload = ""; code = ""; onConnect(known.origin, name, "")
                                }) { Text("重新连接") }
                            }
                        }
                    }
                    item {
                        Text("请在可信局域网使用。HTTP 传输未加密，连接前请核对电脑地址。",
                            style = MaterialTheme.typography.bodySmall, color = RelayColors.Muted, modifier = Modifier.padding(horizontal = 4.dp))
                        Spacer(Modifier.height(8.dp))
                        Text("随手传 ${BuildConfig.VERSION_NAME}", style = MaterialTheme.typography.labelMedium, color = RelayColors.Muted, modifier = Modifier.padding(horizontal = 4.dp))
                    }
                }
            }
            HorizontalDivider(color = RelayColors.Line)
            NavigationBar(containerColor = Color.White, tonalElevation = 0.dp, windowInsets = WindowInsets(0)) {
                listOf("收发" to RelayGlyph.Send, "传输" to RelayGlyph.Transfer, "设备" to RelayGlyph.Devices).forEachIndexed { index, (label, glyph) ->
                    NavigationBarItem(selected = tab == index, onClick = { tab = index },
                        icon = {
                            if (index == 1 && tasks.any { !it.terminal || it.cancelPending }) BadgedBox(badge = { Badge(containerColor = RelayColors.Sage) { Text(tasks.count { !it.terminal || it.cancelPending }.toString()) } }) { RelayIcon(glyph) }
                            else RelayIcon(glyph)
                        }, label = { Text(label) },
                        colors = NavigationBarItemDefaults.colors(selectedIconColor = RelayColors.Sage, selectedTextColor = RelayColors.Sage,
                            indicatorColor = RelayColors.SageLight, unselectedIconColor = RelayColors.Muted, unselectedTextColor = RelayColors.Muted))
                }
            }
        }
    }
    if (projectDialog) SimpleValueDialog("给内容加个项目标签", "项目名称（可选）", project, 120,
        description = "以后发送的内容会自动带上这个标签，方便在电脑与手机上归档。留空即可关闭。",
        onDismiss = { projectDialog = false }, onSave = { project = it; projectDialog = false })
    if (nameDialog) DeviceNameDialog(name, onDismiss = { nameDialog = false }, onSave = { name = it; nameDialog = false })
    feedbackBuild?.let { build ->
        FeedbackDialog(build, feed.filter { it.kind == "file" && it.id != build.id }, busy, onDismiss = { feedbackBuild = null },
            onSend = { body, attachments, id -> model.sendFeedback(build.id, body, attachments, id) { feedbackBuild = null } })
    }
    statusItem?.let { item ->
        AlertDialog(onDismissRequest = { statusItem = null }, title = { Text("记录实际测试状态") },
            text = {
                LazyColumn(Modifier.heightIn(max = 440.dp), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    item { Text(item.name, style = MaterialTheme.typography.titleSmall, maxLines = 3, overflow = TextOverflow.Ellipsis) }
                    item { Text("仅记录你亲自确认的结果。下载成功不会自动标记安装或测试通过。", style = MaterialTheme.typography.bodyMedium) }
                    items(listOf("unverified", "installed", "passed", "failed", "blocked")) { status ->
                        TextButton(enabled = !busy, onClick = { statusItem = null; model.setTestStatus(item, status) }) { Text(testStatusLabel(status)) }
                    }
                }
            }, confirmButton = { TextButton(onClick = { statusItem = null }) { Text("返回") } })
    }
    if (disconnectDialog) AlertDialog(onDismissRequest = { disconnectDialog = false }, title = { Text("断开这台电脑？") },
        text = { Text("传输会暂停，任务和已保存文件保留。下次可从最近连接继续；电脑撤销连接后，需要重新配对。") },
        confirmButton = { TextButton(onClick = { disconnectDialog = false; model.disconnect() }) { Text("断开连接") } },
        dismissButton = { TextButton(onClick = { disconnectDialog = false }) { Text("保持连接") } })
}

@Composable
internal fun RelayHeader(session: Session?, online: Boolean, pending: Boolean, onDevices: () -> Unit) {
    Row(Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 16.dp),
        verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
        Surface(shape = RoundedCornerShape(14.dp), color = RelayColors.Sage) {
            RelayIcon(RelayGlyph.Send, Modifier.padding(10.dp).size(22.dp), Color.White)
        }
        Text("随手传", style = MaterialTheme.typography.headlineSmall, modifier = Modifier.weight(1f))
        Surface(onClick = onDevices, shape = CircleShape, color = if (online && session != null) RelayColors.SageLight else Color.White,
            border = BorderStroke(1.dp, RelayColors.Line)) {
            Row(Modifier.padding(horizontal = 12.dp, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Box(Modifier.size(6.dp).background(if (online && session != null) RelayColors.Sage else RelayColors.Muted, CircleShape))
                Text(when { pending -> "待批准"; session == null -> "未连接"; online -> "已连接"; else -> "待上线" }, style = MaterialTheme.typography.labelMedium)
            }
        }
    }
}

@Composable
private fun SendHero(connected: Boolean, busy: Boolean, project: String, onFiles: () -> Unit, onText: () -> Unit, onProject: () -> Unit) {
    RelayCard {
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            Text("手机 · 电脑", style = MaterialTheme.typography.labelMedium, color = RelayColors.Sage, modifier = Modifier.weight(1f))
            Surface(onClick = onProject, shape = CircleShape, color = RelayColors.Background) {
                Row(Modifier.padding(horizontal = 10.dp, vertical = 7.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                    RelayIcon(RelayGlyph.Folder, Modifier.size(14.dp), RelayColors.Muted)
                    Text(project.ifBlank { "项目标签" }, style = MaterialTheme.typography.labelMedium, color = RelayColors.Muted, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.widthIn(max = 140.dp))
                }
            }
        }
        Text("好内容，随手就到。", style = MaterialTheme.typography.headlineSmall)
        Text("原图、APK、文件和文字\n在你的设备之间，自在流动。", style = MaterialTheme.typography.bodyMedium, color = RelayColors.Muted)
        Button(onClick = onFiles, enabled = !busy, modifier = Modifier.fillMaxWidth().heightIn(min = 54.dp)) {
            RelayIcon(if (connected) RelayGlyph.Plus else RelayGlyph.Devices, Modifier.size(20.dp))
            Spacer(Modifier.width(8.dp))
            Text(if (connected) "选择文件发送" else "先连接电脑")
        }
        TextButton(onClick = onText, modifier = Modifier.fillMaxWidth()) {
            RelayIcon(RelayGlyph.Text, Modifier.size(18.dp)); Spacer(Modifier.width(8.dp)); Text("发一段文字或链接")
        }
    }
}

@Composable
private fun TextComposer(text: String, busy: Boolean, connected: Boolean, onText: (String) -> Unit, onClose: () -> Unit, onSend: () -> Unit) {
    RelayCard {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text("写点什么", style = MaterialTheme.typography.titleMedium, modifier = Modifier.weight(1f))
            IconButton(onClick = onClose, modifier = Modifier.semantics { contentDescription = "收起文字编辑器，保留草稿" }) { RelayIcon(RelayGlyph.Close, Modifier.size(18.dp), RelayColors.Muted) }
        }
        OutlinedTextField(value = text, onValueChange = onText, modifier = Modifier.fillMaxWidth(), minLines = 3, maxLines = 8,
            placeholder = { Text("粘贴文字、链接或日志\n换行与空格都会保留") })
        Button(onClick = onSend, enabled = connected && text.isNotBlank() && !busy, modifier = Modifier.align(Alignment.End)) {
            Text(if (busy) "正在发送…" else "发送文字")
        }
    }
}

@Composable
internal fun RelayCard(modifier: Modifier = Modifier, tone: Color = Color.White, content: @Composable ColumnScope.() -> Unit) {
    Surface(modifier.fillMaxWidth(), shape = RoundedCornerShape(24.dp), color = tone,
        border = BorderStroke(1.dp, if (tone == Color.White) RelayColors.Line else tone)) {
        Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp), content = content)
    }
}

@Composable
internal fun IconTile(glyph: RelayGlyph, background: Color, tint: Color, small: Boolean = false) {
    Surface(shape = RoundedCornerShape(if (small) 13.dp else 16.dp), color = background) {
        RelayIcon(glyph, Modifier.padding(if (small) 10.dp else 12.dp).size(if (small) 22.dp else 24.dp), tint)
    }
}

@Composable
internal fun EmptyCard(glyph: RelayGlyph, title: String, body: String, action: String = "", onAction: () -> Unit = {}) {
    RelayCard {
        Column(Modifier.fillMaxWidth().padding(vertical = 16.dp), horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(14.dp)) {
            IconTile(glyph, RelayColors.Lavender, RelayColors.LilacInk)
            Text(title, style = MaterialTheme.typography.titleMedium)
            Text(body, style = MaterialTheme.typography.bodyMedium, color = RelayColors.Muted)
            if (action.isNotBlank()) TextButton(onClick = onAction) { Text(action) }
        }
    }
}

@Composable
private fun NoticeCard(text: String, onDismiss: () -> Unit) {
    Surface(Modifier.padding(start = 20.dp, end = 20.dp, bottom = 8.dp).fillMaxWidth(), color = RelayColors.SageLight, shape = RoundedCornerShape(16.dp)) {
        Row(Modifier.padding(start = 12.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            RelayIcon(RelayGlyph.Info, Modifier.size(18.dp), RelayColors.Sage)
            Text(text, Modifier.weight(1f).padding(vertical = 12.dp), style = MaterialTheme.typography.bodySmall)
            IconButton(onClick = onDismiss, modifier = Modifier.semantics { contentDescription = "收起提示" }) { RelayIcon(RelayGlyph.Close, Modifier.size(16.dp), RelayColors.Muted) }
        }
    }
}

@Composable
private fun SimpleValueDialog(title: String, label: String, initial: String, max: Int, description: String, onDismiss: () -> Unit, onSave: (String) -> Unit) {
    var value by rememberSaveable { mutableStateOf(initial) }
    AlertDialog(onDismissRequest = onDismiss, title = { Text(title) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(16.dp)) {
                Text(description, style = MaterialTheme.typography.bodyMedium)
                OutlinedTextField(value, onValueChange = { value = it.take(max) }, modifier = Modifier.fillMaxWidth(), label = { Text(label) }, singleLine = true)
            }
        }, confirmButton = { Button(onClick = { onSave(value.trim()) }) { Text("保存") } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("取消") } })
}

@Composable
private fun DeviceNameDialog(initial: String, onDismiss: () -> Unit, onSave: (String) -> Unit) {
    var value by rememberSaveable { mutableStateOf(initial) }
    AlertDialog(onDismissRequest = onDismiss, title = { Text("这台设备叫什么？") },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Text("电脑会显示这个名字，方便你认出连接请求。", style = MaterialTheme.typography.bodyMedium)
                FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    listOf("我的手机", "我的平板", "另一台电脑").forEach { preset ->
                        FilterChip(selected = value == preset, onClick = { value = preset }, label = { Text(preset) })
                    }
                }
                OutlinedTextField(value, onValueChange = { value = it.take(80) }, modifier = Modifier.fillMaxWidth(), label = { Text("自定义名称") }, singleLine = true)
            }
        }, confirmButton = { Button(enabled = value.isNotBlank(), onClick = { onSave(value.trim()) }) { Text("保存") } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("取消") } })
}
