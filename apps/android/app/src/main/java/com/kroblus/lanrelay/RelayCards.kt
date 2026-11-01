package com.kroblus.lanrelay

import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import java.util.UUID
import kotlinx.coroutines.delay

@Composable
internal fun FeedCard(item: FeedItem, busy: Boolean, onCopy: (String) -> Unit, onDownload: () -> Unit, onFeedback: () -> Unit, onStatus: () -> Unit) {
    var details by rememberSaveable(item.id) { mutableStateOf(false) }
    var expanded by rememberSaveable(item.id) { mutableStateOf(false) }
    val glyph = when {
        item.kind == "text" -> RelayGlyph.Text
        item.kind == "feedback" -> RelayGlyph.Edit
        item.isApk -> RelayGlyph.Apk
        item.mime.startsWith("image/") -> RelayGlyph.Image
        item.mime.startsWith("video/") -> RelayGlyph.Video
        else -> RelayGlyph.File
    }
    val tint = when (glyph) { RelayGlyph.Image, RelayGlyph.Text -> RelayColors.LilacInk; else -> RelayColors.Sage }
    val tone = when (glyph) { RelayGlyph.Image, RelayGlyph.Text -> RelayColors.Lavender; RelayGlyph.Video, RelayGlyph.Edit -> RelayColors.Apricot; else -> RelayColors.SageLight }
    RelayCard {
        Row(verticalAlignment = Alignment.Top, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            IconTile(glyph, tone, tint)
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Text(when (item.kind) { "text" -> "一段文字"; "feedback" -> "测试反馈"; else -> item.name },
                    style = MaterialTheme.typography.titleMedium, maxLines = 3, overflow = TextOverflow.Ellipsis)
                val size = if (item.kind == "file") formatBytes(item.size) else if (item.kind == "feedback") "${item.attachments.size} 个附件" else "文字与链接"
                Text(listOf(size, SimpleDateFormat("MM-dd HH:mm", Locale.CHINA).format(Date(item.createdAt))).joinToString(" · "),
                    style = MaterialTheme.typography.bodySmall, color = RelayColors.Muted)
            }
            IconButton(onClick = { details = !details }, modifier = Modifier.size(40.dp).semantics { contentDescription = if (details) "收起内容详情" else "查看内容详情" }) {
                RelayIcon(RelayGlyph.More, Modifier.size(20.dp), RelayColors.Muted)
            }
        }
        if (item.project.isNotBlank() || item.isApk) FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            if (item.project.isNotBlank()) SuggestionChip(onClick = { details = !details }, label = { Text(item.project, maxLines = 1, overflow = TextOverflow.Ellipsis) })
            if (item.isApk) SuggestionChip(onClick = onStatus, enabled = !busy, label = { Text(testStatusShort(item.testStatus)) },
                colors = SuggestionChipDefaults.suggestionChipColors(containerColor = RelayColors.SageLight, labelColor = RelayColors.Sage), border = null)
        }
        if (item.text.isNotEmpty()) {
            SelectionContainer {
                Text(item.text, style = MaterialTheme.typography.bodyLarge, maxLines = if (expanded) Int.MAX_VALUE else 5, overflow = TextOverflow.Ellipsis)
            }
            if (item.text.length > 180 || item.text.count { it == '\n' } > 4)
                TextButton(onClick = { expanded = !expanded }) { Text(if (expanded) "收起文字" else "展开全文") }
        }
        if (item.isApk && item.apkVersion.isNotBlank()) Text("版本 ${item.apkVersion}", style = MaterialTheme.typography.bodySmall, color = RelayColors.Muted)
        FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            if (item.kind == "file" || item.kind == "feedback") FilledTonalButton(onClick = onDownload, enabled = !busy,
                colors = ButtonDefaults.filledTonalButtonColors(containerColor = RelayColors.SageLight, contentColor = RelayColors.Sage)) {
                RelayIcon(RelayGlyph.Down, Modifier.size(18.dp)); Spacer(Modifier.width(6.dp)); Text(if (item.kind == "feedback") "导出反馈" else "保存到手机")
            }
            if (item.text.isNotEmpty()) TextButton(onClick = { onCopy(item.text) }) {
                RelayIcon(RelayGlyph.Copy, Modifier.size(17.dp)); Spacer(Modifier.width(6.dp)); Text("复制")
            }
            if (item.isApk) TextButton(onClick = onFeedback, enabled = !busy) { Text("写反馈") }
        }
        if (details) {
            HorizontalDivider(color = RelayColors.Line)
            Text("内容详情", style = MaterialTheme.typography.titleSmall)
            SelectionContainer {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    if (item.kind == "file") Text("文件：${item.name}\n类型：${item.mime}", style = MaterialTheme.typography.bodySmall, color = RelayColors.Muted)
                    if (item.apkPackage.isNotEmpty()) Text("包名：${item.apkPackage}", style = MaterialTheme.typography.bodySmall)
                    if (item.sha256.isNotEmpty()) Text("SHA-256\n${item.sha256}", style = MaterialTheme.typography.bodySmall, color = RelayColors.Muted)
                    if (item.kind == "feedback") Text("关联构建：${item.buildId}\n附件：${item.attachments.size} 个", style = MaterialTheme.typography.bodySmall)
                    if (item.note.isNotBlank()) Text(item.note, style = MaterialTheme.typography.bodyMedium)
                }
            }
            if (item.sha256.isNotBlank()) TextButton(onClick = { onCopy(item.sha256) }) { Text("复制校验值") }
            if (item.isApk) {
                Text("下载成功不代表已安装或测试通过。", style = MaterialTheme.typography.bodySmall, color = RelayColors.Muted)
                TextButton(onClick = onStatus, enabled = !busy) { Text("记录安装 / 测试状态") }
            }
        }
    }
}

@Composable
internal fun TransferCard(task: TransferTask, metric: TransferMetrics?, busy: Boolean, onResume: () -> Unit, onPause: () -> Unit, onCancel: () -> Unit, onReselect: () -> Unit, onOpen: () -> Unit) {
    val completed = task.status == "complete"
    val moving = task.status == "uploading" || task.status == "downloading"
    val phaseStarted = remember(task.id, task.status) { elapsedMillis() }
    var sampleTime by remember { mutableLongStateOf(elapsedMillis()) }
    // Only the visible network task runs this 1 Hz clock; no background/global animation.
    LaunchedEffect(moving) {
        if (moving) while (true) { sampleTime = elapsedMillis(); delay(1000) }
    }
    val stalled = NetworkProgressPolicy.stalled(metric, sampleTime, phaseStarted)
    val phase = when {
        task.cancelPending -> "电脑端取消待确认"
        completed -> "已完成"
        task.status == "hashing" -> "准备文件 · 计算校验值"
        task.status == "verifying" && task.direction == "upload" -> "电脑正在校验"
        task.status == "verifying" -> "校验并保存"
        else -> task.statusLabel()
    }
    RelayCard {
        Row(verticalAlignment = Alignment.Top, horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            IconTile(if (completed) RelayGlyph.Check else if (task.direction == "upload") RelayGlyph.Up else RelayGlyph.Down,
                if (task.direction == "upload" || completed) RelayColors.SageLight else RelayColors.Lavender,
                if (task.direction == "upload" || completed) RelayColors.Sage else RelayColors.LilacInk, small = true)
            Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Text(task.name, style = MaterialTheme.typography.titleMedium, maxLines = 3, overflow = TextOverflow.Ellipsis)
                Text(if (task.direction == "upload") "发送到电脑" else "接收到手机", style = MaterialTheme.typography.bodySmall, color = RelayColors.Muted)
            }
        }
        Text(phase, style = MaterialTheme.typography.titleSmall,
            color = if (task.status == "failed" || task.status == "needs_file") MaterialTheme.colorScheme.error else RelayColors.Sage)
        if (!task.terminal) {
            if (task.status == "hashing" || task.status == "verifying" || task.status == "saving")
                LinearProgressIndicator(modifier = Modifier.fillMaxWidth().height(5.dp), trackColor = RelayColors.SageLight)
            else if (task.size > 0) LinearProgressIndicator(
                progress = { (task.offset.toFloat() / task.size.toFloat()).coerceIn(0f, 1f) },
                modifier = Modifier.fillMaxWidth().height(5.dp), trackColor = RelayColors.SageLight)
        }
        if (moving) {
            FlowRow(horizontalArrangement = Arrangement.spacedBy(12.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Text(if (stalled) "等待传输…" else formatRate(metric?.bytesPerSecond ?: 0.0), style = MaterialTheme.typography.titleMedium)
                val remaining = if (stalled) "" else formatRemaining(metric?.remainingSeconds)
                if (remaining.isNotBlank()) Text(remaining, style = MaterialTheme.typography.bodySmall, color = RelayColors.Muted, modifier = Modifier.align(Alignment.CenterVertically))
            }
        }
        if (task.status != "hashing") Text(
            if (completed) formatBytes(task.size) else "${formatBytes(task.offset)} / ${formatBytes(task.size)} · 已确认",
            style = MaterialTheme.typography.bodySmall, color = RelayColors.Muted)
        if (task.error.isNotEmpty()) Text(task.error, style = MaterialTheme.typography.bodyMedium,
            color = if (task.status == "failed") MaterialTheme.colorScheme.error else RelayColors.Muted)
        if (!task.terminal) FlowRow(horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            if (task.active || task.status == "queued") TextButton(onClick = onPause, enabled = !busy) {
                RelayIcon(RelayGlyph.Pause, Modifier.size(17.dp)); Spacer(Modifier.width(6.dp)); Text("暂停")
            }
            else if (task.status != "needs_file") FilledTonalButton(onClick = onResume, enabled = !busy) {
                RelayIcon(RelayGlyph.Play, Modifier.size(17.dp)); Spacer(Modifier.width(6.dp)); Text("继续传输")
            }
            if (task.status == "needs_file") FilledTonalButton(onClick = onReselect, enabled = !busy) { Text("重选原文件") }
            TextButton(onClick = onCancel, enabled = !busy) { Text("取消") }
        }
        else if (completed && task.savedUri.isNotEmpty()) TextButton(onClick = onOpen) { Text("打开已保存文件") }
        else if (task.status == "cancelled" && task.cancelPending) TextButton(onClick = onCancel, enabled = !busy) { Text("重试电脑端取消") }
    }
}

@Composable
internal fun FeedbackDialog(build: FeedItem, files: List<FeedItem>, busy: Boolean, onDismiss: () -> Unit, onSend: (String, Set<String>, String) -> Unit) {
    var body by rememberSaveable(build.id) { mutableStateOf("") }
    var selected by remember(build.id) { mutableStateOf(emptySet<String>()) }
    val requestId = remember(build.id, body, selected) { UUID.randomUUID().toString() }
    AlertDialog(onDismissRequest = onDismiss, title = { Text("给这个构建写反馈") },
        text = {
            LazyColumn(Modifier.heightIn(max = 480.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                item { Text(build.name, style = MaterialTheme.typography.titleSmall, maxLines = 3, overflow = TextOverflow.Ellipsis) }
                item {
                    OutlinedTextField(value = body, onValueChange = { body = it.take(32768) }, minLines = 4, maxLines = 10,
                        placeholder = { Text("如何复现？\n预期是什么？实际发生了什么？\n设备与系统版本（确认后填写）") }, modifier = Modifier.fillMaxWidth())
                }
                item {
                    Text("关联截图、录屏或日志", style = MaterialTheme.typography.titleSmall)
                    Text("从已上传文件中选择 · ${selected.size} / 20", style = MaterialTheme.typography.bodySmall, color = RelayColors.Muted)
                }
                if (files.isEmpty()) item { Text("先把附件发送到电脑，再回到这个构建填写反馈。", style = MaterialTheme.typography.bodyMedium, color = RelayColors.Muted) }
                items(files.take(50), key = { it.id }) { file ->
                    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                        Checkbox(checked = file.id in selected, enabled = file.id in selected || selected.size < 20,
                            onCheckedChange = { checked -> selected = if (checked) selected + file.id else selected - file.id })
                        Text(file.name, Modifier.weight(1f), style = MaterialTheme.typography.bodyMedium, maxLines = 3, overflow = TextOverflow.Ellipsis)
                    }
                }
            }
        }, confirmButton = { Button(enabled = body.isNotBlank() && !busy, onClick = { onSend(body, selected, requestId) }) { Text("提交反馈") } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("取消") } })
}

internal fun testStatusShort(status: String) = when (status) {
    "installed" -> "已确认安装"
    "passed" -> "测试通过"
    "failed" -> "测试未通过"
    "blocked" -> "测试受阻"
    else -> "尚未测试"
}

internal fun testStatusLabel(status: String) = when (status) {
    "installed" -> "已人工确认安装"
    "passed" -> "PASS · 测试通过"
    "failed" -> "FAIL · 测试失败"
    "blocked" -> "BLOCKED · 验收受阻"
    else -> "NOT RUN · 尚未验证"
}
