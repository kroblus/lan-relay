package com.kroblus.lanrelay

import java.util.Locale

internal object FileOpenPolicy {
    const val APK_MIME = "application/vnd.android.package-archive"
    fun mime(name: String, supplied: String): String {
        // Windows file imports can legitimately arrive as application/octet-stream.
        // Prefer the actual filename for common formats, especially the APK installer.
        return when (name.substringAfterLast('.', "").lowercase(Locale.ROOT)) {
            "apk" -> APK_MIME
            "png" -> "image/png"
            "jpg", "jpeg" -> "image/jpeg"
            "webp" -> "image/webp"
            "gif" -> "image/gif"
            "heic" -> "image/heic"
            "heif" -> "image/heif"
            "bmp" -> "image/bmp"
            "mp4" -> "video/mp4"
            "pdf" -> "application/pdf"
            "txt", "log", "md" -> "text/plain"
            else -> supplied.substringBefore(';').trim().lowercase(Locale.ROOT)
                .takeIf { it.matches(Regex("[a-z0-9!#$&^_.+-]+/[a-z0-9!#$&^_.+-]+")) } ?: "application/octet-stream"
        }
    }
    fun isApk(task: TransferTask) = mime(task.name, task.mime) == APK_MIME
    fun isImage(task: TransferTask) = mime(task.name, task.mime).startsWith("image/")
    fun openLabel(task: TransferTask) = when {
        isApk(task) -> "安装应用"
        isImage(task) -> "查看图片"
        else -> "打开文件"
    }
    fun previewSize(width: Int, height: Int): Pair<Int, Int> {
        require(width > 0 && height > 0) { "图片尺寸无法读取。" }
        val scale = maxOf(width / 2048.0, height / 2048.0, 1.0)
        return maxOf(1, (width / scale).toInt()) to maxOf(1, (height / scale).toInt())
    }
}
