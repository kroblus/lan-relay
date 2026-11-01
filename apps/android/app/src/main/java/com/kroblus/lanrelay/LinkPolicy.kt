package com.kroblus.lanrelay

import java.net.InetAddress
import java.net.URI
import java.net.URLDecoder

data class PairLink(val origin: String, val inviteToken: String = "")

object LinkPolicy {
    fun parse(input: String): PairLink {
        val value = input.trim()
        require(value.length in 1..4096 && !value.contains('\\') && value.none { it.isISOControl() }) { "请输入电脑显示的局域网地址。" }
        if (value.startsWith("lanrelay:", true)) {
            val link = URI(value)
            require(link.scheme.equals("lanrelay", true) && link.host == "pair" && link.rawFragment == null && link.rawUserInfo == null && link.port == -1 && link.path.orEmpty().isEmpty()) { "无法识别配对二维码。" }
            val parameters = linkedMapOf<String, String>()
            for (entry in link.rawQuery.orEmpty().split('&')) {
                val parts = entry.split('=', limit = 2)
                require(parts.size == 2 && parts[0] in setOf("v", "url", "token") && !parameters.containsKey(parts[0])) { "配对二维码参数无效。" }
                parameters[parts[0]] = URLDecoder.decode(parts[1], "UTF-8")
            }
            require(parameters["v"] == "2" && parameters.keys == setOf("v", "url", "token")) { "请使用新版电脑生成的二维码。" }
            val token = parameters.getValue("token")
            require(token.matches(Regex("[A-Za-z0-9_-]{32,256}"))) { "配对凭据无效，请重新扫码。" }
            return PairLink(origin(parameters.getValue("url")), token)
        }
        return PairLink(origin(value))
    }

    fun origin(input: String): String {
        val bare = !input.contains("://")
        val uri = URI(if (bare) "http://$input" else input)
        require(uri.scheme == "http" || uri.scheme == "https") { "连接地址必须使用 HTTP 或 HTTPS。" }
        require(uri.rawUserInfo == null && uri.rawQuery == null && uri.rawFragment == null && (uri.path.isNullOrEmpty() || uri.path == "/")) { "请使用电脑连接入口，不要粘贴文件链接或管理密钥。" }
        val host = uri.host?.removePrefix("[")?.removeSuffix("]")?.lowercase() ?: throw IllegalArgumentException("电脑地址无效。")
        require(localHost(host)) { "只允许连接局域网电脑；127.0.0.1 指向手机自己。" }
        val port = if (bare && uri.port == -1) 4318 else uri.port
        require(port == -1 || port in 1..65535) { "端口无效。" }
        return URI(uri.scheme, null, host, port, null, null, null).toASCIIString()
    }

    fun localHost(host: String): Boolean {
        if (host == "localhost" || host.endsWith(".localhost") || host.contains('%')) return false
        if (host.contains(':')) return runCatching { privateAddress(InetAddress.getByName(host)) }.getOrDefault(false)
        if (host.matches(Regex("[0-9.]+"))) {
            val pieces = host.split('.')
            if (pieces.size != 4 || pieces.any { it.isEmpty() || (it.length > 1 && it.startsWith('0')) || it.toIntOrNull() !in 0..255 }) return false
            val n = pieces.map { it.toInt() }
            return n[0] == 10 || (n[0] == 172 && n[1] in 16..31) || (n[0] == 192 && n[1] == 168) ||
                (n[0] == 169 && n[1] == 254) || (n[0] == 100 && n[1] in 64..127)
        }
        return host.matches(Regex("[a-z0-9](?:[a-z0-9.-]{0,251}[a-z0-9])?")) &&
            (host.endsWith(".local") || host.endsWith(".lan") || host.endsWith(".home.arpa") || !host.contains('.'))
    }

    fun privateAddress(address: InetAddress): Boolean {
        if (address.isLoopbackAddress || address.isAnyLocalAddress || address.isMulticastAddress) return false
        val b = address.address.map { it.toInt() and 255 }
        return if (b.size == 4) b[0] == 10 || (b[0] == 172 && b[1] in 16..31) || (b[0] == 192 && b[1] == 168) ||
            (b[0] == 169 && b[1] == 254) || (b[0] == 100 && b[1] in 64..127)
        else (b[0] and 254) == 252 || (b[0] == 254 && (b[1] and 192) == 128)
    }

    fun id(value: String): String {
        require(value.matches(Regex("[A-Za-z0-9_-]{1,128}"))) { "电脑返回了无效标识。" }
        return value
    }
}
