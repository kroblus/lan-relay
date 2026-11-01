package com.kroblus.lanrelay

import okhttp3.MediaType
import okhttp3.RequestBody
import okio.BufferedSink

/** Reports bytes written to the request stream; it does not acknowledge remote storage. */
internal class UploadProgressBody(
    private val bytes: ByteArray,
    private val count: Int,
    private val type: MediaType,
    private val progress: (Long) -> Unit
) : RequestBody() {
    init { require(count in 0..bytes.size) }
    override fun contentType(): MediaType = type
    override fun contentLength(): Long = count.toLong()
    override fun writeTo(sink: BufferedSink) {
        var written = 0
        progress(0)
        while (written < count) {
            val length = minOf(64 * 1024, count - written)
            sink.write(bytes, written, length)
            written += length
            progress(written.toLong())
        }
    }
}
