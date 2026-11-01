package com.kroblus.lanrelay

import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import java.io.IOException
import java.net.InetAddress
import java.net.SocketTimeoutException
import javax.net.ssl.SSLHandshakeException

class ExperienceRegressionTest {
    @Test fun staleResumeCannotRequeueCompletedFileOrReviveCancelledTask() {
        val session = Session("http://192.168.1.25:4318", "server", "电脑", "phone", "fixture")
        val task = TransferTask(direction = "download", origin = session.origin, serverId = session.serverId, deviceId = session.deviceId, name = "file", status = "complete", savedUri = "content://media/downloads/123")
        assertEquals(task, TransferRules.resume(task, session))
        assertEquals("cancelled", TransferRules.resume(task.copy(status = "cancelled"), session).status)
        assertEquals("queued", TransferRules.resume(task.copy(status = "paused"), session).status)
        assertEquals("downloading", TransferRules.resume(task.copy(status = "downloading"), session).status)
        assertThrows(IllegalArgumentException::class.java) { TransferRules.resume(task, session.copy(deviceId = "other-device")) }
    }
    @Test fun verifiedPublicationWinsLatePauseOrCancelWithoutLosingSavedUri() {
        val hash = "a".repeat(64)
        val task = TransferTask(direction = "download", origin = "http://192.168.1.25:4318", serverId = "server", deviceId = "phone", name = "large.apk", size = 1_200_000_000, sha256 = hash)
        for (state in listOf("verifying", "paused", "cancelled")) {
            val finished = TransferRules.savedDownload(task.copy(status = state), "content://media/downloads/123", task.size, hash)
            assertEquals("complete", finished.status)
            assertEquals("content://media/downloads/123", finished.savedUri)
            assertEquals(task.size, finished.offset)
        }
        assertThrows(IllegalArgumentException::class.java) { TransferRules.savedDownload(task, "content://media/downloads/123", task.size - 1, hash) }
        assertThrows(IllegalArgumentException::class.java) { TransferRules.savedDownload(task, "content://media/downloads/123", task.size, "b".repeat(64)) }
    }
    @Test fun concurrentSendTapsAcquireOnlyOneSubmission() {
        val gate = DraftSubmissionGate()
        val start = java.util.concurrent.CountDownLatch(1)
        val attempted = java.util.concurrent.CountDownLatch(20)
        val successes = java.util.concurrent.atomic.AtomicInteger()
        val threads = (0 until 20).map {
            Thread { start.await(); if (gate.acquire()) successes.incrementAndGet(); attempted.countDown() }.apply { start() }
        }
        start.countDown()
        assertTrue(attempted.await(3, java.util.concurrent.TimeUnit.SECONDS))
        threads.forEach { it.join(1000) }
        assertEquals(1, successes.get())
        gate.release()
        assertTrue(gate.acquire())
    }
    @Test fun retryKeepsFileRequestIdentityButNewDraftAndTargetDoNot() {
        val source = TransferTask(direction = "upload", origin = "http://192.168.1.25:4318", serverId = "server", deviceId = "phone", uri = "content://fixture/a", name = "a.apk")
        val first = DraftTaskIdentity.bind(source, "draft-one")
        val retry = DraftTaskIdentity.bind(source.copy(id = "new-random-id", requestId = "different-random-request"), "draft-one")
        assertEquals(first.id, retry.id)
        assertEquals(first.requestId, retry.requestId)
        assertTrue(DraftTaskIdentity.sameSource(first, retry))
        assertNotEquals(first.id, DraftTaskIdentity.bind(source, "draft-two").id)
        assertNotEquals(first.id, DraftTaskIdentity.bind(source.copy(serverId = "different-server"), "draft-one").id)
        assertFalse(DraftTaskIdentity.sameSource(first, first.copy(uri = "content://fixture/b")))
    }
    @Test fun windowsApkMimeUsesPackageInstallerType() {
        assertEquals(FileOpenPolicy.APK_MIME, FileOpenPolicy.mime("我的应用.APK", "application/octet-stream"))
        assertEquals("image/jpeg", FileOpenPolicy.mime("原图.JPG", "application/octet-stream"))
        assertEquals("application/octet-stream", FileOpenPolicy.mime("data", "text/plain\r\nInvalid: header"))
    }
    @Test fun imagePreviewIsBoundedWithoutChangingOriginalDimensions() {
        assertEquals(2048 to 1024, FileOpenPolicy.previewSize(24_000, 12_000))
        assertEquals(400 to 800, FileOpenPolicy.previewSize(400, 800))
        assertEquals(1 to 2048, FileOpenPolicy.previewSize(1, 100_000))
        assertThrows(IllegalArgumentException::class.java) { FileOpenPolicy.previewSize(0, 200) }
    }
    @Test fun retriesOnlyTransientTransportAndNeverAuthTlsOrDiskFailures() {
        val transient = DownloadTransportFailure(SocketTimeoutException("read timed out"))
        assertEquals(750L, DownloadRetryPolicy.delayMillis(transient, 0))
        assertEquals(1750L, DownloadRetryPolicy.delayMillis(transient, 1))
        assertEquals(3500L, DownloadRetryPolicy.delayMillis(transient, 2))
        assertNull(DownloadRetryPolicy.delayMillis(transient, 3))
        assertNull(DownloadRetryPolicy.delayMillis(ApiFailure(401, "REVOKED", "revoked"), 0))
        assertNull(DownloadRetryPolicy.delayMillis(ApiFailure(403, "FORBIDDEN", "forbidden"), 0))
        assertNull(DownloadRetryPolicy.delayMillis(DownloadTransportFailure(SSLHandshakeException("untrusted")), 0))
        assertNull(DownloadRetryPolicy.delayMillis(IOException("No space left on device"), 0))
        assertNull(DownloadRetryPolicy.delayMillis(DownloadIntegrityFailure("bad hash"), 0))
        assertEquals(750L, DownloadRetryPolicy.delayMillis(ApiFailure(503, "BUSY", "retry"), 0))
    }
    @Test fun discoveryDerivesOriginOnlyFromPrivatePacketSource() {
        val id = "3e3d8c42-713e-40c2-a2cd-8a72a18336cd"
        val packet = JSONObject().put("magic", "lanrelay").put("v", 1).put("kind", "announce")
            .put("peer_id", id).put("name", "我的电脑").put("role", "desktop").put("port", 4318)
        val address = InetAddress.getByName("192.168.1.25")
        fun parse() = DiscoveryProtocol.parse(packet.toString().toByteArray(), address, 500)
        assertEquals("http://192.168.1.25:4318", parse()!!.origin)
        assertEquals(id, parse()!!.id)
        assertFalse(parse()!!.invited)
        packet.put("kind", "connect")
        assertTrue(parse()!!.invited)
        assertNull(DiscoveryProtocol.parse(packet.toString().toByteArray(), InetAddress.getByName("8.8.8.8"), 500))
        assertNull(DiscoveryProtocol.parse(ByteArray(1201), address, 500))
        packet.put("url", "http://8.8.8.8:4318")
        assertNull(parse())
        packet.remove("url")
        packet.put("name", "wrong\nname")
        assertNull(parse())
    }
    @Test fun discoveryVerificationSurvivesRestartAndOldPairingStillLoads() {
        val pending = PendingPair("http://192.168.1.25:4318", "server", "电脑", "request", "claim", 1000, "003982")
        assertEquals(pending, PendingPair.parse(pending.json()))
        val legacy = pending.json().apply { remove("verification_code") }
        assertEquals("", PendingPair.parse(legacy).verificationCode)
        assertFalse(PendingPair.parse(legacy).cancelRequested)
        val cancelledOffline = pending.copy(cancelRequested = true)
        assertTrue(PendingPair.parse(cancelledOffline.json()).cancelRequested)
    }
    @Test fun automaticRetryStillPausesAfterProcessRestart() {
        val task = TransferTask(direction = "download", origin = "http://192.168.1.25:4318", serverId = "server", deviceId = "device", name = "large.apk", status = "retrying", offset = 1_200_000_000)
        assertTrue(task.active)
        assertEquals("paused", task.afterProcessRestart().status)
        assertEquals(1_200_000_000L, task.afterProcessRestart().offset)
    }
}
