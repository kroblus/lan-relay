package com.kroblus.lanrelay

enum class CameraPermissionStep { OPEN_SCANNER, REQUEST, EXPLAIN, SETTINGS }

/** Pure decision logic; the Activity always checks the current Android permission before launching. */
object CameraPermissionRules {
    fun next(granted: Boolean, shouldExplain: Boolean, requestedBefore: Boolean): CameraPermissionStep = when {
        granted -> CameraPermissionStep.OPEN_SCANNER
        shouldExplain -> CameraPermissionStep.EXPLAIN
        requestedBefore -> CameraPermissionStep.SETTINGS
        else -> CameraPermissionStep.REQUEST
    }
}
