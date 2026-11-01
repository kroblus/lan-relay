package com.kroblus.lanrelay

import okhttp3.MediaType.Companion.toMediaType
import org.junit.Assert.*
import org.junit.Test
import java.io.ByteArrayInputStream
import java.io.ByteArrayOutputStream
import java.io.InputStream
import java.io.OutputStream
import java.security.MessageDigest

class TransferPerformanceTest {
    private fun sample() = TransferTask(direction = "upload", origin = "http://192.168.1.2:4318",
        serverId = "pc", deviceId = "phone", name = "source.apk", size = 1024L * 1024 * 1024,
        uri = "content://source/file", status = "uploading", uploadId = "u", sha256 = "a".repeat(64))

    @Test fun OnlyCachedOffsetsAndProgressMessagesCanAvoidImmediateCommit() {
        val old = sample()
        assertFalse(ProgressJournalPolicy.criticalChange(old, old.copy(offset = 2097152, error = "")))
        assertFalse(ProgressJournalPolicy.criticalChange(old.copy(status = "hashing"), old.copy(status = "hashing", error = "正在校验 8 MiB")))
        for (critical in listOf(
            old.copy(uploadId = "another"), old.copy(requestId = "new"), old.copy(sha256 = "b".repeat(64)),
            old.copy(uri = "content://source/other"), old.copy(size = 1), old.copy(origin = "http://192.168.1.3:4318"),
            old.copy(deviceId = "other"), old.copy(serverId = "other"), old.copy(status = "paused"),
            old.copy(status = "cancelled", cancelPending = true), old.copy(status = "complete"),
            old.copy(status = "failed"), old.copy(status = "verifying"), old.copy(savedUri = "content://downloads/new")
        )) assertTrue("Must persist " + critical, ProgressJournalPolicy.criticalChange(old, critical))
    }

    @Test fun ProgressCheckpointsHaveTimeByteAndRewindBounds() {
        assertFalse(ProgressJournalPolicy.checkpointDue(100, 0, 1099, 2097152))
        assertTrue(ProgressJournalPolicy.checkpointDue(100, 0, 1100, 2097152))
        assertTrue(ProgressJournalPolicy.checkpointDue(100, 0, 101, 32L * 1024 * 1024))
        assertTrue(ProgressJournalPolicy.checkpointDue(100, 8192, 101, 0))
    }

    @Test fun CoalescedJournalCutsWritesAndRetainsTheFinalDurableState() {
        var writes = 0
        var lastTime = 0L
        var saved = sample()
        var memory = saved
        // Same 1 GiB stream, 2 MiB acknowledgements, 25 ms/ACK (80 MiB/s).
        for (ack in 1..512) {
            memory = memory.copy(offset = ack * 2L * 1024 * 1024)
            val now = ack * 25L
            if (ProgressJournalPolicy.checkpointDue(lastTime, saved.offset, now, memory.offset)) {
                saved = memory
                lastTime = now
                writes++
            }
        }
        assertEquals(32, writes)
        assertEquals(memory.offset, saved.offset)
        assertEquals(memory.uploadId, saved.uploadId)
        assertEquals(memory.sha256, saved.sha256)
        assertTrue(ProgressJournalPolicy.criticalChange(memory, memory.copy(status = "complete")))
    }

    @Test fun MeterStartsAtResumeOffsetAndDoesNotCountAlreadyTransferredBytes() {
        val meter = ThroughputMeter(16L * 1024 * 1024, 8L * 1024 * 1024, 50_000)
        assertEquals(0.0, meter.sample(8L * 1024 * 1024, 50_100).bytesPerSecond, 0.0)
        val sample = meter.sample(10L * 1024 * 1024, 50_500)
        assertEquals(4.0 * 1024 * 1024, sample.bytesPerSecond, 0.0)
        assertEquals(500L, sample.elapsedMillis)
        assertEquals(1L, sample.remainingSeconds)
    }

    @Test fun StallRequiresNewNetworkBytesToClearAndDoesNotReuseOldEta() {
        val meter = ThroughputMeter(16L * 1024 * 1024, 0, 10_000)
        val moving = meter.sample(2L * 1024 * 1024, 10_500)
        assertFalse(NetworkProgressPolicy.stalled(moving, 14_499, 10_000))
        assertTrue(NetworkProgressPolicy.stalled(moving, 14_500, 10_000))
        val unchanged = meter.sample(2L * 1024 * 1024, 15_000)
        assertTrue(NetworkProgressPolicy.stalled(unchanged, 15_000, 10_000))
        val resumed = meter.sample(4L * 1024 * 1024, 15_500)
        assertFalse(NetworkProgressPolicy.stalled(resumed, 15_500, 10_000))
        assertTrue(NetworkProgressPolicy.stalled(null, 14_000, 10_000))
        assertNull(ThroughputMeter(-1, 0, 1000).sample(1024, 1500).remainingSeconds)
    }

    @Test fun SlowContinuousNetworkStaysActiveBeforeFirstDurableCheckpoint() {
        val start = 10_000L
        val telemetry = NetworkTelemetry(8L * 1024 * 1024, 0, start)
        var visible: TransferMetrics? = null
        // 128 KiB/s: a 2 MiB durability checkpoint takes 16 seconds, while
        // real bytes continue arriving every 250 ms throughout that interval.
        for (step in 1..64) {
            val now = start + step * 250L
            telemetry.observe(step * 32L * 1024, now)?.let { visible = it }
            assertFalse("Active stream falsely stalled at ${now - start} ms", NetworkProgressPolicy.stalled(visible, now, start))
            if (step >= 2) assertEquals(128.0 * 1024, visible!!.bytesPerSecond, 0.01)
        }
    }

    @Test fun IdleCallbacksCannotKeepAStoppedConnectionAlive() {
        val start = 10_000L
        val telemetry = NetworkTelemetry(8L * 1024 * 1024, 0, start)
        val visible = telemetry.observe(64 * 1024L, start + 500)!!
        assertNull(telemetry.observe(64 * 1024L, start + 4500))
        assertFalse(NetworkProgressPolicy.stalled(visible, start + 4499, start))
        assertTrue(NetworkProgressPolicy.stalled(visible, start + 4500, start))
        val recovered = telemetry.observe(96 * 1024L, start + 5000)!!
        assertFalse(NetworkProgressPolicy.stalled(recovered, start + 5000, start))
    }

    @Test fun NetworkTelemetryPublishesAtMostTwicePerSecond() {
        val start = 10_000L
        val telemetry = NetworkTelemetry(100L * 1024 * 1024, 0, start)
        var publications = 0
        for (step in 1..500) if (telemetry.observe(step * 4096L, start + step * 20L) != null) publications++
        assertEquals(20, publications)
    }

    @Test fun UploadProgressIsBoundedAndCanCancelBeforeSendingTheNextBytes() {
        val bytes = ByteArray(150 * 1024) { it.toByte() }
        val sink = okio.Buffer()
        val calls = mutableListOf<Long>()
        val body = UploadProgressBody(bytes, bytes.size, "application/octet-stream".toMediaType()) { written ->
            calls += written
            if (written == 64 * 1024L) throw TransferPaused()
        }
        assertThrows(TransferPaused::class.java) { body.writeTo(sink) }
        assertEquals(listOf(0L, 64 * 1024L), calls)
        assertEquals(64 * 1024L, sink.size)
        assertArrayEquals(bytes.copyOf(64 * 1024), sink.readByteArray())
    }

    @Test fun SinglePassCopyHashesExactlyTheWrittenBytes() {
        val bytes = ("中文内容\n".repeat(100_000)).toByteArray()
        val out = ByteArrayOutputStream()
        var checkpoints = 0
        val result = TransferIo.copyAndHash(ByteArrayInputStream(bytes), out) { checkpoints++ }
        assertEquals(bytes.size.toLong(), result.first)
        assertArrayEquals(bytes, out.toByteArray())
        assertEquals(MessageDigest.getInstance("SHA-256").digest(bytes).toHex(), result.second)
        assertTrue(checkpoints > 1)
    }

    @Test fun EmptyAndShortReadsRemainValidWithoutFileSizedAllocations() {
        val bytes = ByteArray(4097) { it.toByte() }
        val short = object : ByteArrayInputStream(bytes) {
            override fun read(target: ByteArray, offset: Int, length: Int): Int = super.read(target, offset, minOf(length, 7))
        }
        val result = TransferIo.copyAndHash(short, ByteArrayOutputStream())
        assertEquals(bytes.size.toLong(), result.first)
        assertEquals(MessageDigest.getInstance("SHA-256").digest(bytes).toHex(), result.second)
        val empty = TransferIo.copyAndHash(ByteArrayInputStream(byteArrayOf()), ByteArrayOutputStream())
        assertEquals(0L, empty.first)
        assertEquals(MessageDigest.getInstance("SHA-256").digest().toHex(), empty.second)
    }

    @Test fun CancellationStopsBeforePublishingMoreUnverifiedBytes() {
        var writes = 0L
        var checks = 0
        val out = object : OutputStream() {
            override fun write(value: Int) { writes++ }
            override fun write(bytes: ByteArray, offset: Int, length: Int) { writes += length }
        }
        assertThrows(IllegalStateException::class.java) {
            TransferIo.copyAndHash(ByteArrayInputStream(ByteArray(1024 * 1024)), out) {
                if (++checks == 3) throw IllegalStateException("pause")
            }
        }
        assertEquals(512 * 1024L, writes)
    }

    @Test fun SinglePassVerificationReadsLargeSourceOnceWithBoundedBuffers() {
        val length = 32L * 1024 * 1024
        var remaining = length
        var readBytes = 0L
        var maxRequested = 0
        val source = object : InputStream() {
            override fun read(): Int = error("Must use bounded buffers")
            override fun read(bytes: ByteArray, offset: Int, count: Int): Int {
                maxRequested = maxOf(maxRequested, count)
                if (remaining == 0L) return -1
                val n = minOf(remaining, count.toLong()).toInt()
                bytes.fill(0x5a, offset, offset + n)
                remaining -= n
                readBytes += n
                return n
            }
        }
        val sink = object : OutputStream() {
            override fun write(value: Int) {}
            override fun write(bytes: ByteArray, offset: Int, length: Int) {}
        }
        val result = TransferIo.copyAndHash(source, sink)
        assertEquals(length, result.first)
        assertEquals(length, readBytes)
        assertEquals(256 * 1024, maxRequested)
    }
}
