package com.kroblus.lanrelay

import java.io.InputStream
import java.io.OutputStream
import java.security.MessageDigest
import kotlin.math.max

/** In-memory network activity, never a durable offset or pre-hash/disk-copy progress. */
data class TransferMetrics(val bytesPerSecond: Double = 0.0, val remainingSeconds: Long? = null, val elapsedMillis: Long = 0,
    val lastProgressAt: Long = 0)

internal class ThroughputMeter(private val total: Long, offset: Long, private val started: Long) {
    private var sampledAt = started
    private var sampledOffset = offset
    private var rate = 0.0
    private var observedOffset = offset
    private var lastProgressAt = started
    fun sample(offset: Long, now: Long): TransferMetrics {
        if (offset > observedOffset) {
            observedOffset = offset
            lastProgressAt = now
        }
        val delta = now - sampledAt
        if (delta >= 250 && offset >= sampledOffset) {
            val current = (offset - sampledOffset) * 1000.0 / delta
            rate = if (rate == 0.0) current else rate * 0.35 + current * 0.65
            sampledAt = now
            sampledOffset = offset
        }
        return TransferMetrics(rate, if (rate > 0.0 && total >= 0) (max(0L, total - offset) / rate).toLong() else null,
            max(0, now - started), lastProgressAt)
    }
}

/** Throttle UI telemetry independently of fsync / server acknowledgements. */
internal class NetworkTelemetry(total: Long, offset: Long, started: Long) {
    private val meter = ThroughputMeter(total, offset, started)
    private var lastOffset = offset
    private var publishedAt = started

    fun observe(offset: Long, now: Long): TransferMetrics? {
        // A timer or repeated callback must never make an idle connection look active.
        if (offset <= lastOffset) return null
        lastOffset = offset
        val sample = meter.sample(offset, now)
        if (now - publishedAt < 500) return null
        publishedAt = now
        return sample
    }
}

internal object NetworkProgressPolicy {
    fun stalled(metric: TransferMetrics?, now: Long, phaseStarted: Long): Boolean =
        now - (metric?.lastProgressAt?.takeIf { it > 0 } ?: phaseStarted) >= 4000
}

/** Only cached progress is coalesced. Upload identity and all state changes are committed immediately. */
internal object ProgressJournalPolicy {
    const val INTERVAL_MILLIS = 1000L
    const val BYTE_INTERVAL = 32L * 1024 * 1024
    fun criticalChange(old: TransferTask, next: TransferTask): Boolean =
        !next.active || old.copy(offset = next.offset, error = next.error) != next
    fun checkpointDue(lastTime: Long, lastOffset: Long, now: Long, offset: Long): Boolean =
        now - lastTime >= INTERVAL_MILLIS || offset - lastOffset >= BYTE_INTERVAL || offset < lastOffset
}

internal object TransferIo {
    /** Hashes the same bounded buffers written to a still-private destination. */
    fun copyAndHash(input: InputStream, output: OutputStream, checkpoint: () -> Unit = {}): Pair<Long, String> {
        val hash = MessageDigest.getInstance("SHA-256")
        val buffer = ByteArray(256 * 1024)
        var total = 0L
        while (true) {
            checkpoint()
            val count = input.read(buffer)
            if (count < 0) break
            if (count == 0) continue
            output.write(buffer, 0, count)
            hash.update(buffer, 0, count)
            total += count
        }
        output.flush()
        return total to hash.digest().toHex()
    }
}

internal fun elapsedMillis(): Long = System.nanoTime() / 1_000_000

fun formatRate(value: Double): String = when {
    value <= 0.0 -> "测速中"
    value < 1024 * 1024 -> "%.0f KiB/s".format(value / 1024)
    else -> "%.1f MiB/s".format(value / (1024 * 1024))
}

fun formatRemaining(seconds: Long?): String = when {
    seconds == null -> ""
    seconds < 1 -> "即将完成"
    seconds < 60 -> "约 " + seconds + " 秒"
    else -> "约 " + ((seconds + 59) / 60) + " 分钟"
}
