package com.kroblus.lanrelay

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.IBinder
import androidx.core.app.NotificationCompat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

class TransferService : Service() {
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private lateinit var store: RelayStore
    private var worker: Job? = null
    private var engine: TransferEngine? = null
    private var runningId: String? = null
    private var lastNotificationTime = 0L
    private var lastNotificationState = ""
    override fun onBind(intent: Intent?): IBinder? = null
    override fun onCreate() {
        super.onCreate()
        store = (application as RelayApp).store
        getSystemService(NotificationManager::class.java).createNotificationChannel(
            NotificationChannel(CHANNEL, "文件传输", NotificationManager.IMPORTANCE_LOW).apply { description = "显示当前传输，可随时暂停。" })
        scope.launch {
            store.tasks.collectLatest { tasks ->
                val active = tasks.firstOrNull { it.id == runningId }
                if (active?.status in setOf("paused", "cancelled")) engine?.cancel()
                val now = elapsedMillis()
                val state = active?.let { it.id + ":" + it.status }.orEmpty()
                if (worker?.isActive == true && (state != lastNotificationState || now - lastNotificationTime >= 500)) {
                    getSystemService(NotificationManager::class.java).notify(NOTIFICATION, notification(active))
                    lastNotificationState = state
                    lastNotificationTime = now
                }
            }
        }
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == PAUSE_ALL) {
            store.pauseAll()
            engine?.cancel()
            stopForeground(STOP_FOREGROUND_REMOVE)
            stopSelf()
            return START_NOT_STICKY
        }
        // User initiated from visible Activity; never restart from BOOT_COMPLETED or a background timer.
        startForeground(NOTIFICATION, notification(null), ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC)
        if (worker?.isActive != true) worker = scope.launch {
            try {
                while (true) {
                    val session = store.session.value ?: break
                    val task = store.tasks.value.firstOrNull { it.status == "queued" && it.serverId == session.serverId && it.deviceId == session.deviceId && it.origin == session.origin } ?: break
                    runningId = task.id
                    val current = TransferEngine(this@TransferService, store, session)
                    engine = current
                    withContext(Dispatchers.IO) { current.run(task) }
                    runningId = null
                    engine = null
                }
            } finally {
                runningId = null
                engine = null
                stopForeground(STOP_FOREGROUND_REMOVE)
                stopSelf()
            }
        }
        return START_NOT_STICKY
    }

    override fun onTimeout(startId: Int, fgsType: Int) {
        store.pauseAll("系统后台传输时限已到，请打开应用后继续。")
        engine?.cancel()
        stopForeground(STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    override fun onDestroy() {
        engine?.cancel()
        store.pauseAll("后台任务已停止，请点继续恢复。")
        scope.cancel()
        super.onDestroy()
    }

    private fun notification(task: TransferTask?): Notification {
        val open = PendingIntent.getActivity(this, 0, Intent(this, MainActivity::class.java), PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
        val pause = PendingIntent.getService(this, 1, Intent(this, TransferService::class.java).setAction(PAUSE_ALL), PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
        val progress = if (task != null && task.size > 0) ((task.offset.toDouble() / task.size) * 1000).toInt().coerceIn(0, 1000) else 0
        return NotificationCompat.Builder(this, CHANNEL)
            .setSmallIcon(android.R.drawable.stat_sys_upload)
            .setContentTitle(task?.name ?: "随手传 · 准备传输")
            .setContentText(if (task == null) "正在准备队列" else "${task.statusLabel()} · ${formatBytes(task.offset)} / ${formatBytes(task.size)}")
            .setContentIntent(open).setOnlyAlertOnce(true).setOngoing(true)
            .setProgress(1000, progress, task == null || task.status in setOf("hashing", "verifying", "saving"))
            .addAction(0, "暂停全部", pause).build()
    }
    companion object {
        const val CHANNEL = "transfers-v2"
        const val NOTIFICATION = 203
        const val PAUSE_ALL = "com.kroblus.lanrelay.PAUSE_ALL"
    }
}

fun TransferTask.statusLabel(): String = when (status) {
    "queued" -> "等待传输"
    "hashing" -> "校验原文件"
    "uploading" -> "发送中"
    "downloading" -> "接收中"
    "verifying" -> "校验并保存"
    "saving" -> "保存文件"
    "paused" -> "已暂停"
    "needs_file" -> "需要重选原文件"
    "complete" -> "已完成"
    "cancelled" -> "已取消"
    else -> "待重试"
}
