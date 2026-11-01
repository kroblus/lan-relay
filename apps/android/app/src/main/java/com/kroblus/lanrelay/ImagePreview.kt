package com.kroblus.lanrelay

import android.graphics.ImageDecoder
import android.net.Uri
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.*
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.withContext
import java.io.FileNotFoundException

@Composable
internal fun ImagePreview(task: TransferTask, onDismiss: () -> Unit, onOtherApp: () -> Unit) {
    val context = LocalContext.current
    var bitmap by remember(task.id) { mutableStateOf<ImageBitmap?>(null) }
    var failure by remember(task.id) { mutableStateOf("") }
    LaunchedEffect(task.id, task.savedUri) {
        runCatching {
            withContext(Dispatchers.IO) {
                val source = ImageDecoder.createSource(context.contentResolver, Uri.parse(task.savedUri))
                ImageDecoder.decodeBitmap(source) { decoder, info, _ ->
                    val (width, height) = FileOpenPolicy.previewSize(info.size.width, info.size.height)
                    decoder.setTargetSize(width, height)
                    decoder.allocator = ImageDecoder.ALLOCATOR_SOFTWARE
                }.asImageBitmap()
            }
        }.onSuccess { bitmap = it }.onFailure {
            if (it is CancellationException) throw it
            failure = when (it) {
                is FileNotFoundException -> "图片可能被移动或删除，请返回内容详情重新保存一份。"
                is SecurityException -> "系统拒绝了这张图片的读取授权，请重新保存一份。"
                else -> "这张图片暂时无法预览，可用其他应用打开。"
            }
        }
    }
    Dialog(onDismissRequest = onDismiss, properties = DialogProperties(usePlatformDefaultWidth = false)) {
        Surface(Modifier.fillMaxSize().safeDrawingPadding(), color = RelayColors.Background) {
            Column(Modifier.fillMaxSize().padding(16.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(task.name, Modifier.weight(1f), style = MaterialTheme.typography.titleMedium, maxLines = 2, overflow = TextOverflow.Ellipsis)
                    TextButton(onClick = onDismiss) { Text("关闭") }
                }
                Box(Modifier.weight(1f).fillMaxWidth(), contentAlignment = Alignment.Center) {
                    bitmap?.let { Image(it, contentDescription = task.name, modifier = Modifier.fillMaxSize(), contentScale = ContentScale.Fit) }
                        ?: if (failure.isBlank()) CircularProgressIndicator() else Text(failure)
                }
                if (bitmap != null) Text("原图已保存 · 预览不会改变原文件", style = MaterialTheme.typography.bodySmall, color = RelayColors.Muted)
                TextButton(onClick = onOtherApp, modifier = Modifier.align(Alignment.End)) { Text("用其他应用打开") }
            }
        }
    }
}
