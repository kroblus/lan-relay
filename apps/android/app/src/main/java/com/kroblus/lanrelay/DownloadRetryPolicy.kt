package com.kroblus.lanrelay

import java.io.IOException
import javax.net.ssl.SSLException

/** Only transport errors are wrapped; disk, validation and permission failures are not retried. */
internal class DownloadTransportFailure(cause: IOException) : IOException(cause.message, cause)

internal object DownloadRetryPolicy {
    fun delayMillis(failure: Throwable, retries: Int): Long? {
        if (retries !in 0..2) return null
        val recoverable = when (failure) {
            is ApiFailure -> failure.status in setOf(408, 429, 502, 503, 504)
            is DownloadTransportFailure -> failure.cause !is SSLException
            else -> false
        }
        return if (recoverable) longArrayOf(750, 1750, 3500)[retries] else null
    }
}
