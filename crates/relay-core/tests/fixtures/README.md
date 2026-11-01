# Manifest fixture

`v0_2_manifest.b64` is the base64-encoded binary Android manifest extracted from the previously generated `lan-relay-0.2.0-android-trial.apk`, source revision `3475f56a6598b1e8561ef2b23c655d2d5de83361`. The decoded manifest SHA-256 is `ccd3743584654f906f9a58e7b022d9a8652fe27451ebe1d9c7684ce123c007a0`.

It tests bounded binary-XML parsing against an actual Android build output. It contains only application manifest metadata; no signing key, session token or user content. The test does not establish that an APK is installable, correctly signed or tested on a physical device.
