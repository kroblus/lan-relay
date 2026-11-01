package com.kroblus.lanrelay

import org.junit.Assert.*
import org.junit.Assume.assumeTrue
import org.junit.Test
import java.io.Closeable
import java.net.Inet4Address
import java.net.InetAddress
import java.net.NetworkInterface
import java.net.ServerSocket
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit

/** Real HTTP requests against an isolated local fixture; no user files, credentials or relay server. */
class RelayApiTest {
    @Test fun bearerCredentialStaysInHeader() {
        Fixture(200, """{"items":[],"next_cursor":null}""").use { server ->
            RelayApi(server.origin, "fixture-session-token").json("/api/v2/items?limit=50")
            val headers = server.request.poll(3, TimeUnit.SECONDS) ?: error("No HTTP request")
            assertEquals("GET /api/v2/items?limit=50 HTTP/1.1", headers.first())
            assertTrue(headers.any { it.equals("Authorization: Bearer fixture-session-token", ignoreCase = true) })
            assertFalse(headers.first().contains("fixture-session-token"))
        }
    }

    @Test fun publicHandshakeDoesNotSendCredential() {
        Fixture(200, """{"protocol_version":2,"server_id":"fixture","name":"电脑","max_file_bytes":1024,"chunk_size":512}""").use { server ->
            assertEquals("fixture", RelayApi(server.origin, "fixture-session-token").hello().serverId)
            val headers = server.request.poll(3, TimeUnit.SECONDS) ?: error("No HTTP request")
            assertFalse(headers.any { it.startsWith("Authorization:", ignoreCase = true) })
        }
    }

    @Test fun changedServerIdentityBlocksCredentialsAndWrite() {
        Fixture(200, """{"protocol_version":2,"server_id":"different-pc","name":"另一台电脑","max_file_bytes":1024,"chunk_size":512}""").use { server ->
            val session = Session(server.origin, "original-pc", "原电脑", "device", "fixture-session-token")
            assertThrows(IllegalArgumentException::class.java) {
                RelayApi.verified(session).json("/api/v2/text", "POST", org.json.JSONObject().put("text", "private draft"))
            }
            val headers = server.request.poll(3, TimeUnit.SECONDS) ?: error("No HTTP request")
            assertEquals("GET /api/v2/hello HTTP/1.1", headers.first())
            assertFalse(headers.any { it.startsWith("Authorization:", ignoreCase = true) })
            assertEquals(0, server.body.poll(3, TimeUnit.SECONDS)!!.size)
            assertTrue(server.request.isEmpty())
        }
    }

    @Test fun authenticatedRedirectIsRejected() {
        Fixture(302, "", mapOf("Location" to "http://192.168.254.254:9/stolen")).use { server ->
            val failure = assertThrows(ApiFailure::class.java) { RelayApi(server.origin, "fixture-session-token").json("/api/v2/items") }
            assertEquals("REDIRECT_REJECTED", failure.code)
            assertEquals(302, failure.status)
        }
    }

    @Test fun revokedDeviceErrorIsPreserved() {
        Fixture(401, """{"error":{"code":"UNAUTHORIZED","message":"设备已撤销"}}""").use { server ->
            val failure = assertThrows(ApiFailure::class.java) { RelayApi(server.origin, "fixture-session-token").json("/api/v2/items") }
            assertEquals(401, failure.status)
            assertEquals("UNAUTHORIZED", failure.code)
            assertEquals("设备已撤销", failure.message)
        }
    }

    @Test fun sendsOnlyRequestedBinaryChunkWithOffset() {
        Fixture(200, """{"id":"u1","offset":7,"size":10,"chunk_size":4,"status":"uploading","sha256":"abc"}""").use { server ->
            val result = RelayApi(server.origin, "fixture-session-token").sendChunk("u1", 4, byteArrayOf(1, 2, 3, 99), 3)
            assertEquals(7L, result.offset)
            val headers = server.request.poll(3, TimeUnit.SECONDS) ?: error("No HTTP request")
            assertTrue(headers.any { it == "Upload-Offset: 4" })
            assertTrue(headers.any { it == "Content-Length: 3" })
            assertArrayEquals(byteArrayOf(1, 2, 3), server.body.poll(3, TimeUnit.SECONDS))
        }
    }

    @Test fun downloadResumeUsesServerShaEtagAndRange() {
        val hash = "d".repeat(64)
        Fixture(206, "remaining", mapOf("Content-Range" to "bytes 4-12/13", "ETag" to "\"sha256-$hash\"")).use { server ->
            RelayApi(server.origin, "fixture-session-token").execute("/api/v2/files/file1", headers = TransferRules.downloadHeaders(4, hash)).use { response ->
                assertEquals(206, response.code)
                assertEquals("remaining", response.body!!.string())
            }
            val headers = server.request.poll(3, TimeUnit.SECONDS) ?: error("No HTTP request")
            assertTrue(headers.any { it == "Range: bytes=4-" })
            assertTrue(headers.any { it == "If-Range: \"sha256-$hash\"" })
        }
        assertTrue(TransferRules.downloadHeaders(0, hash).isEmpty())
    }

    private class Fixture(status: Int, responseBody: String, responseHeaders: Map<String, String> = emptyMap()) : Closeable {
        val request = LinkedBlockingQueue<List<String>>()
        val body = LinkedBlockingQueue<ByteArray>()
        private val server: ServerSocket
        private val thread: Thread
        val origin: String
        init {
            val address = NetworkInterface.getNetworkInterfaces().toList().filter { it.isUp }.flatMap { it.inetAddresses.toList() }
                .firstOrNull { it is Inet4Address && LinkPolicy.privateAddress(it) }
            assumeTrue("HTTP fixture needs a private network interface; no public network is contacted.", address != null)
            server = ServerSocket(0, 4, InetAddress.getByName("0.0.0.0"))
            server.soTimeout = 5000
            origin = "http://${address!!.hostAddress}:${server.localPort}"
            thread = Thread {
                runCatching {
                    server.accept().use { socket ->
                        socket.soTimeout = 5000
                        val input = socket.getInputStream().buffered()
                        fun line(): String {
                            val result = StringBuilder()
                            while (true) { val c = input.read(); if (c < 0 || c == 10) break; if (c != 13) result.append(c.toChar()) }
                            return result.toString()
                        }
                        val lines = mutableListOf<String>()
                        while (true) { val current = line(); if (current.isEmpty()) break; lines += current }
                        val length = lines.firstOrNull { it.startsWith("Content-Length:", true) }?.substringAfter(':')?.trim()?.toInt() ?: 0
                        val bytes = ByteArray(length)
                        var received = 0
                        while (received < length) { val count = input.read(bytes, received, length - received); if (count < 0) break; received += count }
                        request.put(lines)
                        body.put(bytes)
                        val payload = responseBody.toByteArray(Charsets.UTF_8)
                        val response = buildString {
                            append("HTTP/1.1 $status Fixture\r\nContent-Type: application/json\r\nContent-Length: ${payload.size}\r\nConnection: close\r\n")
                            responseHeaders.forEach { (key, value) -> append("$key: $value\r\n") }
                            append("\r\n")
                        }
                        socket.getOutputStream().apply { write(response.toByteArray(Charsets.UTF_8)); write(payload); flush() }
                    }
                }
            }.apply { isDaemon = true; start() }
        }
        override fun close() { server.close(); thread.join(1000) }
    }
}
