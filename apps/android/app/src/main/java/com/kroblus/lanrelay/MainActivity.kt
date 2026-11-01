package com.kroblus.lanrelay

import android.Manifest
import android.content.ClipData
import android.content.ClipboardManager
import android.content.Intent
import android.content.ActivityNotFoundException
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.provider.Settings
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.viewModels
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.core.content.ContextCompat
import androidx.lifecycle.lifecycleScope
import com.journeyapps.barcodescanner.ScanContract
import com.journeyapps.barcodescanner.ScanOptions
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.FileNotFoundException

class MainActivity : ComponentActivity() {
    private val model: RelayViewModel by viewModels()
    private var permissionAction: (() -> Unit)? = null
    private var replacementId: String? = null
    private var selectedProject = ""
    private var cameraPrompt by mutableStateOf<CameraPermissionStep?>(null)
    private var preview by mutableStateOf<TransferTask?>(null)
    private var installPrompt by mutableStateOf<TransferTask?>(null)
    private var pendingInstallId: String? = null
    private var openingFile = false
    private val installPermission = registerForActivityResult(ActivityResultContracts.StartActivityForResult()) {
        val task = pendingInstallId?.let(model.store::task)
        pendingInstallId = null
        if (task != null && packageManager.canRequestPackageInstalls()) openExternal(task)
        else model.message("尚未允许随手传安装应用。文件已保留，可再次点击安装。")
    }
    private val cameraPermission = registerForActivityResult(ActivityResultContracts.RequestPermission()) { granted ->
        if (granted && cameraGranted()) openScanner()
        else cameraPrompt = if (shouldShowRequestPermissionRationale(Manifest.permission.CAMERA)) CameraPermissionStep.EXPLAIN else CameraPermissionStep.SETTINGS
    }
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
            else if (replace != null) model.enqueueFiles(uris, selectedProject, replace, ::startTransfers)
            else model.stageFiles(uris)
        }
    }
    private val scanner = registerForActivityResult(ScanContract()) { result ->
        result.contents?.let { model.incomingAddress.value = it }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        pendingInstallId = savedInstanceState?.getString("pending_install_id")
        if (savedInstanceState == null) receiveIntent(intent)
        setContent {
            RelayTheme {
                RelayScreen(model,
                    onConnect = { address, name, code -> permissions(false) { model.connect(address, name, code) } },
                    onScan = ::requestScan,
                    onDiscover = { permissions(false) { model.startDiscovery() } },
                    onDiscovered = { peer, name -> permissions(false) { model.connectDiscovered(peer, name) } },
                    onRequireNetwork = { action -> permissions(false, action) },
                    onPick = { project, replacement -> selectedProject = project; replacementId = replacement; filePicker.launch(arrayOf("*/*")) },
                    onStartTransfers = ::startTransfers,
                    onCopy = ::copy,
                    onOpen = ::openFile)
                preview?.let { task -> ImagePreview(task, onDismiss = { preview = null }, onOtherApp = { openExternal(task, choose = true) }) }
                installPrompt?.let { task ->
                    AlertDialog(onDismissRequest = { installPrompt = null },
                        title = { Text("允许安装这份应用") },
                        text = { Text("首次安装需要在系统设置中允许「随手传」安装应用。允许后会返回系统安装页面，由你确认安装。") },
                        confirmButton = { TextButton(onClick = {
                            installPrompt = null
                            pendingInstallId = task.id
                            runCatching { installPermission.launch(Intent(Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES, Uri.parse("package:$packageName"))) }
                                .onFailure { pendingInstallId = null; model.message("无法打开安装权限设置，请在系统的应用设置中允许安装未知应用。") }
                        }) { Text("去允许") } },
                        dismissButton = { TextButton(onClick = { installPrompt = null }) { Text("稍后") } })
                }
                cameraPrompt?.let { prompt ->
                    AlertDialog(onDismissRequest = { cameraPrompt = null },
                        title = { Text("允许相机，连接电脑") },
                        text = { Text(if (prompt == CameraPermissionStep.SETTINGS) "相机权限尚未开启。请在应用设置中允许相机，然后回来点击扫码；也可以使用电脑地址和连接码手动连接。" else "扫描电脑上的配对二维码需要相机权限。相机只用于本次扫码；你也可以使用手动连接。") },
                        confirmButton = { TextButton(onClick = {
                            cameraPrompt = null
                            if (prompt == CameraPermissionStep.SETTINGS) startActivity(Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse("package:$packageName")))
                            else requestCameraPermission()
                        }) { Text(if (prompt == CameraPermissionStep.SETTINGS) "打开设置" else "允许扫码") } },
                        dismissButton = { TextButton(onClick = { cameraPrompt = null }) { Text("暂不允许") } })
                }
            }
        }
    }
    override fun onSaveInstanceState(outState: Bundle) {
        outState.putString("pending_install_id", pendingInstallId)
        super.onSaveInstanceState(outState)
    }
    override fun onStart() { super.onStart(); model.visible(true); if (localPermissionGranted()) model.startDiscovery() }
    override fun onStop() { model.visible(false); model.stopDiscovery(); super.onStop() }
    override fun onNewIntent(intent: Intent) { super.onNewIntent(intent); setIntent(intent); receiveIntent(intent) }

    private fun cameraGranted() = ContextCompat.checkSelfPermission(this, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED
    private fun requestScan() {
        val next = CameraPermissionRules.next(cameraGranted(), shouldShowRequestPermissionRationale(Manifest.permission.CAMERA),
            model.store.preferences.getBoolean("camera_requested", false))
        when (next) {
            CameraPermissionStep.OPEN_SCANNER -> openScanner()
            CameraPermissionStep.REQUEST -> requestCameraPermission()
            else -> cameraPrompt = next
        }
    }
    private fun requestCameraPermission() {
        model.store.preferences.edit().putBoolean("camera_requested", true).apply()
        cameraPermission.launch(Manifest.permission.CAMERA)
    }
    private fun openScanner() {
        // Recheck immediately before launch: permissions may change while a dialog is open.
        if (!cameraGranted()) { requestScan(); return }
        scanner.launch(ScanOptions().setDesiredBarcodeFormats(ScanOptions.QR_CODE)
            .setPrompt("扫描电脑窗口中的配对二维码").setBeepEnabled(false).setOrientationLocked(false))
    }

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
        if (task.status != "complete" || Uri.parse(task.savedUri).scheme != "content") {
            model.message("文件尚未保存完成，请先等待传输和校验完成。")
            return
        }
        when {
            FileOpenPolicy.isImage(task) -> preview = task
            FileOpenPolicy.isApk(task) && !packageManager.canRequestPackageInstalls() -> installPrompt = task
            else -> openExternal(task)
        }
    }
    private fun openExternal(task: TransferTask, choose: Boolean = false) {
        if (openingFile) return
        openingFile = true
        lifecycleScope.launch {
        try {
            val uri = Uri.parse(task.savedUri)
            withContext(Dispatchers.IO) {
                contentResolver.openFileDescriptor(uri, "r")?.use { }
                    ?: throw FileNotFoundException("Saved file is unavailable")
            }
            val view = Intent(Intent.ACTION_VIEW).setDataAndType(uri, FileOpenPolicy.mime(task.name, task.mime))
                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            // ClipData carries the same read-only grant to the system installer and
            // OEM viewers. Direct ACTION_VIEW honors system defaults on later opens.
            view.clipData = ClipData.newRawUri(task.name, uri)
            startActivity(if (choose) Intent.createChooser(view, "用其他应用打开") else view)
        } catch (_: ActivityNotFoundException) { model.message("没有可打开此类型的应用。文件保存在下载/LANRelay，可安装对应应用后重试。") }
          catch (_: SecurityException) { model.message("系统拒绝了文件访问或安装授权。请检查应用权限；文件记录仍保留，可重新保存一份后再试。") }
          catch (_: FileNotFoundException) { model.message("已保存的文件可能被移动或删除，请在内容详情中重新保存一份。") }
          catch (error: CancellationException) { throw error }
          catch (_: Exception) { model.message("文件暂时无法打开。请在下载/LANRelay 中检查文件，或重新保存一份。") }
          finally { openingFile = false }
        }
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
                model.stageFiles(uris.filter { it.scheme == "content" }.take(100))
                model.sharedText.value = incoming.getCharSequenceExtra(Intent.EXTRA_TEXT)?.toString().orEmpty().take(65536)
                model.message("已接收分享内容，请确认目标电脑后发送。")
            }
        }
    }
}
