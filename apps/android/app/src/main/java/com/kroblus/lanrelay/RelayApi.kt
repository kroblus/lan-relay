package com.kroblus.lanrelay

import okhttp3.Call
import okhttp3.Dns
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody
import okhttp3.RequestBody.Companion.toRequestBody
import okhttp3.Response
import org.json.JSONObject
import java.io.ByteArrayOutputStream
import java.io.IOException
import java.net.InetAddress
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference

class ApiFailure(val status: Int, val code: String, message: String) : IOException(message)

class RelayApi(origin: String, private val token: String = "", controlTimeoutMillis: Long = 120_000) {
    companion object {
        fun verified(session: Session): RelayApi = RelayApi(session.origin, session.token).also { it.verifyServer(session.serverId) }
    }
    val origin = LinkPolicy.origin(origin)
    private val running = AtomicReference<Call?>(null)
    private val client = OkHttpClient.Builder()
        .connectTimeout(12, TimeUnit.SECONDS).readTimeout(45, TimeUnit.SECONDS).writeTimeout(45, TimeUnit.SECONDS)
        .callTimeout(controlTimeoutMillis, TimeUnit.MILLISECONDS)
        .followRedirects(false).followSslRedirects(false)
        .dns(object : Dns {
            override fun lookup(hostname: String): List<InetAddress> {
                val addresses = Dns.SYSTEM.lookup(hostname)
                if (addresses.isEmpty() || addresses.any { !LinkPolicy.privateAddress(it) }) throw IOException("电脑域名解析到了非局域网地址，已停止连接。")
                return addresses
            }
        }).build()

    fun cancel() { running.get()?.cancel() }

    fun execute(path: String, method: String = "GET", body: RequestBody? = null, headers: Map<String, String> = emptyMap(), authenticated: Boolean = true): Response {
        require(path.startsWith("/api/v2/") && !path.contains("..") && !path.contains('\\'))
        val builder = Request.Builder().url(origin + path).header("Accept", "application/json")
            .header("User-Agent", "LANRelay-Android/${BuildConfig.VERSION_NAME}")
        if (authenticated) {
            require(token.isNotEmpty()) { "请先连接电脑。" }
            builder.header("Authorization", "Bearer $token")
        }
        headers.forEach { (key, value) -> builder.header(key, value) }
        builder.method(method, body)
        val call = client.newCall(builder.build())
        // A healthy multi-gigabyte download may take much longer than two minutes.
        // Keep connect/read idle limits and explicit cancellation; only remove the
        // whole-call deadline for the bounded, authenticated file response stream.
        if (method == "GET" && path.startsWith("/api/v2/files/")) call.timeout().clearTimeout()
        running.set(call)
        val response = call.execute()
        if (response.code in 300..399) {
            response.close()
            throw ApiFailure(response.code, "REDIRECT_REJECTED", "电脑返回了跳转地址，已停止发送凭据。请确认电脑连接地址。")
        }
        return response
    }

    fun json(path: String, method: String = "GET", payload: JSONObject? = null, authenticated: Boolean = true): JSONObject {
        val body = when {
            payload != null -> payload.toString().toRequestBody("application/json; charset=utf-8".toMediaType())
            method == "POST" || method == "PATCH" -> "{}".toRequestBody("application/json".toMediaType())
            else -> null
        }
        return execute(path, method, body, authenticated = authenticated).use { response -> parseResponse(response) }
    }

    fun parseResponse(response: Response): JSONObject {
        val raw = response.body ?: throw IOException("电脑返回了空响应。")
        val limit = 8 * 1024 * 1024
        if (raw.contentLength() > limit) throw IOException("电脑返回的数据过大，请减少内容后重试。")
        val output = ByteArrayOutputStream()
        raw.byteStream().use { stream ->
            val buffer = ByteArray(16 * 1024)
            while (true) {
                val count = stream.read(buffer)
                if (count < 0) break
                if (output.size() + count > limit) throw IOException("电脑返回的数据超过限制。")
                output.write(buffer, 0, count)
            }
        }
        val result = runCatching { JSONObject(output.toString("UTF-8")) }.getOrElse {
            if (!response.isSuccessful) throw ApiFailure(response.code, "HTTP_ERROR", "电脑请求失败（HTTP ${response.code}）。")
            throw IOException("电脑返回的响应格式不正确，请检查两端版本。")
        }
        if (!response.isSuccessful) {
            val error = result.optJSONObject("error")
            throw ApiFailure(response.code, error?.string("code") ?: "HTTP_ERROR",
                error?.string("message")?.take(300)?.ifBlank { null } ?: "电脑请求失败（HTTP ${response.code}）。")
        }
        return result
    }

    fun hello(): Hello = Hello.parse(json("/api/v2/hello", authenticated = false))
    fun verifyServer(expected: String): Hello = hello().also { require(it.serverId == expected) { "这个地址对应的电脑身份已改变，请重新核对后配对。" } }
    fun upload(id: String) = RemoteUpload.parse(json("/api/v2/uploads/${LinkPolicy.id(id)}"))
    fun sendChunk(id: String, offset: Long, bytes: ByteArray, count: Int, onProgress: ((Long) -> Unit)? = null): RemoteUpload {
        val type = "application/octet-stream".toMediaType()
        val body = if (onProgress == null) bytes.toRequestBody(type, 0, count) else UploadProgressBody(bytes, count, type, onProgress)
        return execute("/api/v2/uploads/${LinkPolicy.id(id)}", "PATCH", body, mapOf("Upload-Offset" to offset.toString())).use {
            RemoteUpload.parse(parseResponse(it))
        }
    }
}
