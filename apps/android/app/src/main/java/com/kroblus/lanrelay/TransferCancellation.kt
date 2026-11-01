package com.kroblus.lanrelay

import org.json.JSONObject

/** Only an explicit cancel reaches this path; unresolved cancellations retain their server IDs. */
object TransferCancellation {
    fun synchronize(store: RelayStore, task: TransferTask, session: Session) {
        if (!task.cancelPending) return
        TransferRules.validateIdentity(task, session)
        val api = RelayApi.verified(session)
        var id = task.uploadId
        if (id.isEmpty()) {
            // The create response may have been lost. Idempotent replay retrieves that reservation.
            require(task.size >= 0 && task.sha256.matches(Regex("[0-9a-f]{64}"))) { "取消尚未确认，请保留任务记录。" }
            val payload = JSONObject().put("name", task.name).put("size", task.size).put("mime", task.mime).put("sha256", task.sha256)
                .put("project", task.project).put("request_id", task.requestId)
            if (task.buildId.isNotEmpty()) payload.put("build_id", task.buildId)
            id = RemoteUpload.parse(api.json("/api/v2/uploads", "POST", payload)).id
            store.update(task.id) { it.copy(uploadId = id) }
        }
        try {
            api.execute("/api/v2/uploads/${LinkPolicy.id(id)}", "DELETE").use { if (!it.isSuccessful) api.parseResponse(it) }
            store.update(task.id) { it.copy(cancelPending = false, error = "已取消，电脑端已确认。") }
        } catch (error: ApiFailure) {
            if (error.status == 404) {
                store.update(task.id) { it.copy(cancelPending = false, error = "已取消，电脑上已无此临时任务。") }
            } else if (error.code == "UPLOAD_COMPLETE") {
                val item = FeedItem.parse(api.json("/api/v2/uploads/${LinkPolicy.id(id)}/complete", "POST"))
                require(item.sha256 == task.sha256 && item.size == task.size) { "电脑完成文件与任务不一致。" }
                store.update(task.id) { it.copy(status = "complete", offset = item.size, itemId = item.id, cancelPending = false, error = "取消前文件已完成，电脑上的文件已保留。") }
            } else throw error
        }
    }
}
