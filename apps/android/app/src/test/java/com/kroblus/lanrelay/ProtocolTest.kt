package com.kroblus.lanrelay

import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import java.net.URLEncoder

class ProtocolTest {
    @Test fun bareLanAddressUsesDefaultPort() {
        assertEquals("http://192.168.1.20:4318", LinkPolicy.parse("192.168.1.20").origin)
        assertEquals("https://192.168.1.20", LinkPolicy.parse("https://192.168.1.20/").origin)
    }
    @Test fun qrHasExactVersionAndCredential() {
        val token = "a".repeat(43)
        val address = URLEncoder.encode("http://192.168.1.20:4318", "UTF-8")
        assertEquals(PairLink("http://192.168.1.20:4318", token), LinkPolicy.parse("lanrelay://pair?v=2&url=$address&token=$token"))
    }
    @Test fun rejectsPublicLoopbackAndAmbiguousNumericAddresses() {
        listOf("127.0.0.1", "localhost", "8.8.8.8", "192.168.001.2", "192.168.1", "0.0.0.0", "[::1]", "[2001:4860:4860::8888]")
            .forEach { value -> assertThrows(IllegalArgumentException::class.java) { LinkPolicy.parse(value) } }
    }
    @Test fun acceptsPrivateAddressesAndLocalNames() {
        listOf("10.0.0.1", "172.16.0.1", "192.168.2.3", "100.64.1.3", "169.254.1.3", "desktop.local", "[fd00::4]:4318")
            .forEach { assertTrue(LinkPolicy.parse(it).origin.isNotBlank()) }
    }
    @Test fun rejectsCredentialsAndFileLinks() {
        listOf("http://admin@192.168.1.1:4318", "http://192.168.1.1:4318/api/v2/files/a", "http://192.168.1.1:4318/?token=x", "http://192.168.1.1:4318/#owner=secret", "file:///a")
            .forEach { assertThrows(IllegalArgumentException::class.java) { LinkPolicy.parse(it) } }
    }
    @Test fun qrRejectsDuplicateParametersAndOldProtocol() {
        val base = "lanrelay://pair?v=2&url=http%3A%2F%2F192.168.1.3%3A4318&token=${"a".repeat(43)}"
        assertThrows(IllegalArgumentException::class.java) { LinkPolicy.parse("$base&v=2") }
        assertThrows(IllegalArgumentException::class.java) { LinkPolicy.parse(base.replace("v=2", "v=1")) }
        assertThrows(IllegalArgumentException::class.java) { LinkPolicy.parse("$base&owner=secret") }
    }
    @Test fun rejectsIdentifiersThatEscapeRoutes() {
        listOf("../file", "a?token=x", "a/b", "%2F", "").forEach { assertThrows(IllegalArgumentException::class.java) { LinkPolicy.id(it) } }
        assertEquals("a-123_bc", LinkPolicy.id("a-123_bc"))
    }
    @Test fun helloRejectsProtocolMismatchAndBoundsChunkSize() {
        val hello = JSONObject("""{"protocol_version":2,"server_id":"pc","name":"电脑","max_file_bytes":2147483648,"chunk_size":4194304}""")
        assertEquals(4 * 1024 * 1024, Hello.parse(hello).chunkSize)
        hello.put("protocol_version", 3)
        assertThrows(IllegalArgumentException::class.java) { Hello.parse(hello) }
    }
    @Test fun chunkNegotiationKeepsLegacyLimitsAndBoundsLargerServers() {
        val hello = JSONObject("""{"protocol_version":2,"server_id":"pc","name":"电脑","max_file_bytes":2147483648,"chunk_size":2097152}""")
        val upload = JSONObject("""{"id":"u","offset":0,"size":2147483648,"chunk_size":2097152,"status":"uploading","sha256":"abc"}""")
        for ((server, expected) in listOf(2 to 2, 8 to 8, 16 to 8)) {
            hello.put("chunk_size", server * 1024 * 1024)
            upload.put("chunk_size", server * 1024 * 1024)
            assertEquals(expected * 1024 * 1024, minOf(Hello.parse(hello).chunkSize, RemoteUpload.parse(upload).chunkSize))
        }
        upload.put("chunk_size", 2 * 1024 * 1024)
        assertEquals(2 * 1024 * 1024, minOf(Hello.parse(hello).chunkSize, RemoteUpload.parse(upload).chunkSize))
        hello.put("chunk_size", 2 * 1024 * 1024)
        upload.put("chunk_size", 8 * 1024 * 1024)
        assertEquals(2 * 1024 * 1024, minOf(Hello.parse(hello).chunkSize, RemoteUpload.parse(upload).chunkSize))
        for (invalid in listOf(0, -1, 16 * 1024 * 1024 + 1)) {
            hello.put("chunk_size", invalid)
            upload.put("chunk_size", invalid)
            assertThrows(IllegalArgumentException::class.java) { Hello.parse(hello) }
            assertThrows(IllegalArgumentException::class.java) { RemoteUpload.parse(upload) }
        }
    }
    @Test fun validatesCommittedOffsets() {
        val upload = JSONObject("""{"id":"upload","offset":5,"size":8,"chunk_size":4,"status":"uploading","sha256":"abc"}""")
        assertEquals(5L, RemoteUpload.parse(upload).offset)
        upload.put("offset", 9)
        assertThrows(IllegalArgumentException::class.java) { RemoteUpload.parse(upload) }
    }
    @Test fun preservesChineseMultilineAndUnknownApkMetadata() {
        val item = FeedItem.parse(JSONObject().put("id", "file-id").put("kind", "file").put("name", "测试.apk")
            .put("text", "第一行\n第二行\n").put("apk", JSONObject().put("package_name", JSONObject.NULL)))
        assertEquals("第一行\n第二行\n", item.text)
        assertEquals("", item.apkPackage)
        assertTrue(item.isApk)
    }
    @Test fun activeTasksBecomePausedAfterRestart() {
        val task = sample().copy(status = "uploading", offset = 400, uploadId = "u1", sha256 = "c".repeat(64))
        val restored = TransferTask.parse(task.json()).afterProcessRestart()
        assertEquals("paused", restored.status)
        assertEquals(400L, restored.offset)
        assertEquals(task.uploadId, restored.uploadId)
        assertEquals(task.requestId, restored.requestId)
        assertEquals(task.sha256, restored.sha256)
    }
    @Test fun completedAndCancelledTasksAreNotRestarted() {
        assertEquals("complete", sample().copy(status = "complete").afterProcessRestart().status)
        assertEquals("cancelled", sample().copy(status = "cancelled").afterProcessRestart().status)
    }
    @Test fun resumeRequiresSameDeviceServerAndOrigin() {
        val task = sample()
        val session = Session(task.origin, task.serverId, "PC", task.deviceId, "secret")
        TransferRules.validateIdentity(task, session)
        assertThrows(IllegalArgumentException::class.java) { TransferRules.validateIdentity(task, session.copy(deviceId = "new-device")) }
        assertThrows(IllegalArgumentException::class.java) { TransferRules.validateIdentity(task, session.copy(serverId = "another-server")) }
        assertThrows(IllegalArgumentException::class.java) { TransferRules.validateIdentity(task, session.copy(origin = "http://192.168.1.5:4318")) }
    }
    @Test fun resumeRejectsSameSizeDifferentFile() {
        TransferRules.validateSource(10, "abc", 10, "abc")
        assertThrows(IllegalArgumentException::class.java) { TransferRules.validateSource(10, "abc", 10, "def") }
        assertThrows(IllegalArgumentException::class.java) { TransferRules.validateSource(10, "abc", 11, "abc") }
    }
    @Test fun savedNamesCannotEscapeDownloadDirectory() {
        assertEquals(".._secret_file.apk", TransferRules.safeName("../secret\\file.apk"))
        assertFalse(TransferRules.safeName("foo\nbar").contains('\n'))
        assertEquals(180, TransferRules.safeName("x".repeat(400)).length)
    }
    @Test fun taskJournalNeverContainsSessionCredential() {
        val serialized = sample().json().toString()
        assertFalse(serialized.contains("token"))
        assertEquals(sample().direction, TransferTask.parse(sample().json()).direction)
    }
    @Test fun offlineCancellationSurvivesRestartAndCannotBeCleared() {
        val task = sample().copy(status = "paused", uploadId = "u1", offset = 400, sha256 = "c".repeat(64))
        val cancelled = TransferTask.parse(TransferRules.markCancelled(task).json()).afterProcessRestart()
        assertEquals("cancelled", cancelled.status)
        assertTrue(cancelled.cancelPending)
        assertFalse(cancelled.clearable)
        assertEquals(task.uploadId, cancelled.uploadId)
        assertEquals(task.requestId, cancelled.requestId)
        assertEquals(task.offset, cancelled.offset)
        assertTrue(cancelled.copy(cancelPending = false).clearable)
    }
    @Test fun lostCreateResponseStillRetainsIdempotentCancellationAfterRestart() {
        val task = sample().copy(status = "uploading", sha256 = "a".repeat(64))
        val cancelled = TransferRules.markCancelled(TransferTask.parse(task.json()).afterProcessRestart())
        assertTrue(cancelled.cancelPending)
        assertEquals("", cancelled.uploadId)
        assertEquals(task.requestId, cancelled.requestId)
        assertEquals(task.sha256, cancelled.sha256)
        assertFalse(cancelled.clearable)
    }
    @Test fun cancellingBeforeUploadReservationNeedsNoRemoteCleanup() {
        assertFalse(TransferRules.markCancelled(sample()).cancelPending)
        assertTrue(TransferRules.markCancelled(sample().copy(status = "hashing")).clearable)
        assertFalse(TransferRules.markCancelled(sample().copy(direction = "download", sha256 = "a".repeat(64))).cancelPending)
    }
    private fun sample() = TransferTask(direction = "upload", origin = "http://192.168.1.2:4318", serverId = "server", deviceId = "device", uri = "content://documents/a", name = "测试.apk", size = 1000)
}
