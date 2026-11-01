package com.kroblus.lanrelay

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Test

class CameraPermissionTest {
    @Test fun onlyGrantedPermissionCanLaunchScanner() {
        for (rationale in listOf(false, true)) for (requested in listOf(false, true)) {
            assertEquals(CameraPermissionStep.OPEN_SCANNER, CameraPermissionRules.next(true, rationale, requested))
            assertNotEquals(CameraPermissionStep.OPEN_SCANNER, CameraPermissionRules.next(false, rationale, requested))
        }
    }
    @Test fun firstScanRequestsNormalRuntimePermission() {
        assertEquals(CameraPermissionStep.REQUEST, CameraPermissionRules.next(false, false, false))
    }
    @Test fun denialWithRationaleExplainsBeforeAnotherRequest() {
        assertEquals(CameraPermissionStep.EXPLAIN, CameraPermissionRules.next(false, true, true))
    }
    @Test fun PermanentDenialOffersApplicationSettings() {
        assertEquals(CameraPermissionStep.SETTINGS, CameraPermissionRules.next(false, false, true))
    }
}
