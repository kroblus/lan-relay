package com.kroblus.lanrelay;

import org.junit.Test;
import java.net.URLEncoder;
import static org.junit.Assert.*;

public class LinkPolicyTest {
    private static final String TOKEN = "abcdefghijklmnop0123456789_-";

    @Test public void acceptsActualLanPairingWithoutChangingSecrets() {
        LinkPolicy.Endpoint parsed = LinkPolicy.connection("http://192.168.1.20:4318/#pair=" + TOKEN + "&code=123456");
        assertEquals("http://192.168.1.20:4318", parsed.origin);
        assertEquals(parsed.origin + "/#pair=" + TOKEN + "&code=123456", parsed.url);
        assertTrue(parsed.pairedLink);
    }

    @Test public void onlyBareAddressGetsDefaultPort() {
        assertEquals("http://192.168.1.20:4318", LinkPolicy.connection("192.168.1.20").origin);
        assertEquals("http://192.168.1.20", LinkPolicy.connection("http://192.168.1.20").origin);
        assertEquals("https://computer.local:4443", LinkPolicy.connection("https://computer.local:4443/").origin);
    }

    @Test public void deepLinksContainOneEncodedHttpTarget() throws Exception {
        String target = "http://10.0.0.4:4318/#pair=" + TOKEN + "&code=012345";
        assertEquals(target, LinkPolicy.connection("lanrelay://connect?url=" + URLEncoder.encode(target, "UTF-8")).url);
        rejects("lanrelay://connect?url=" + URLEncoder.encode(target, "UTF-8") + "&url=http://10.0.0.5");
        rejects("lanrelay://other?url=" + URLEncoder.encode(target, "UTF-8"));
        rejects("lanrelay://connect?url=javascript%3Aalert(1)");
    }

    @Test public void rejectsRemoteLoopbackAmbiguousAndCredentialAddresses() {
        String[] values = {"https://example.com", "http://127.0.0.1:4318", "http://localhost:4318", "http://[::1]:4318", "http://8.8.8.8:4318", "http://192.168.1.20.evil.com", "http://192.168.1.20@evil.com", "http://user@192.168.1.20", "http://0300.0250.1.1", "http://192.168.1.999", "http://10.0.0.1:65536", "http://10.0.0.1:0", "javascript:alert(1)", "file:///etc/passwd", "http://10.0.0.1/api/files/abc", "http://10.0.0.1/?owner=secret", "http://10.0.0.1\\@evil.com"};
        for (String value : values) rejects(value);
        rejects("http://0x08080808:4318");
        rejects("http://0x7f000001:4318");
    }

    @Test public void ownerAndDuplicateFragmentParametersAreNeverMobileCredentials() {
        rejects("http://10.0.0.1:4318/#owner=" + TOKEN);
        rejects("http://10.0.0.1:4318/#pair=" + TOKEN + "&owner=" + TOKEN);
        rejects("http://10.0.0.1:4318/#code=000000&code=111111");
        rejects("http://10.0.0.1:4318/#pair=short");
        rejects("http://10.0.0.1:4318/#code=12345");
        rejects("http://10.0.0.1:4318/#x=1");
    }

    @Test public void sharedOwnerLinksHaveAnExplicitRejectionEvenForLocalhostOrEncodedKeys() throws Exception {
        String[] values = {"http://10.0.0.1:4318/#owner=" + TOKEN,
                "http://localhost:4318/#owner=" + TOKEN,
                "http://127.0.0.1:4318/#%6fWNER=" + TOKEN,
                "http://10.0.0.1:4318/#unsupported=value&owner=" + TOKEN,
                "lanrelay://connect?url=" + URLEncoder.encode("http://10.0.0.1:4318/#owner=" + TOKEN, "UTF-8")};
        for (String value : values) {
            LinkPolicy.OwnerLinkException error = assertThrows(LinkPolicy.OwnerLinkException.class,
                    () -> LinkPolicy.connection(value));
            assertFalse(error.getMessage().contains(TOKEN));
        }
        IllegalArgumentException ordinaryText = assertThrows(IllegalArgumentException.class,
                () -> LinkPolicy.connection("请与 owner 核对测试结果。"));
        assertFalse(ordinaryText instanceof LinkPolicy.OwnerLinkException);
    }

    @Test public void acceptsPrivateIpv6AndVpnButNotPublicIpv6() {
        assertEquals("http://[fd12:3456::1]:4318", LinkPolicy.connection("http://[fd12:3456::1]:4318").origin);
        assertEquals("http://100.64.1.2:4318", LinkPolicy.connection("http://100.64.1.2:4318").origin);
        rejects("http://[2001:4860:4860::8888]:4318");
        rejects("http://100.128.1.2:4318");
    }

    @Test public void originIncludesSchemeHostAndEffectivePort() {
        assertTrue(LinkPolicy.sameOrigin("http://192.168.1.20:4318", "http://192.168.1.20:4318/app.mjs"));
        assertTrue(LinkPolicy.sameOrigin("https://computer.local", "https://COMPUTER.local:443/"));
        assertFalse(LinkPolicy.sameOrigin("http://192.168.1.20:4318", "http://192.168.1.20:4319/"));
        assertFalse(LinkPolicy.sameOrigin("http://192.168.1.20:4318", "https://192.168.1.20:4318/"));
        assertFalse(LinkPolicy.sameOrigin("http://192.168.1.20:4318", "http://user@192.168.1.20:4318/"));
        assertFalse(LinkPolicy.sameOrigin("http://192.168.1.20:4318", "http://192.168.1.20.evil.com:4318/"));
    }

    @Test public void authenticatedDownloadsAreRestrictedToExactServiceEndpoints() {
        String origin = "http://10.0.0.1:4318";
        assertTrue(LinkPolicy.authenticatedDownload(origin, origin + "/api/files/abc-123"));
        assertTrue(LinkPolicy.authenticatedDownload(origin, origin + "/api/feedback/abc-123/export"));
        String[] paths = {"/api/files/abc/preview", "/api/files/../owner", "/api/files/%2e%2e", "/api/files/abc?url=https://evil.com", "/api/files/abc#x", "/api/session", "/"};
        for (String path : paths) assertFalse(path, LinkPolicy.authenticatedDownload(origin, origin + path));
        assertFalse(LinkPolicy.authenticatedDownload(origin, "http://10.0.0.2:4318/api/files/abc"));
    }

    @Test public void savedNamesCannotEscapeDownloadsOrSpoofWithControlCharacters() {
        assertEquals("_.._a_b.apk", LinkPolicy.safeDownloadName("../../a\nb.apk"));
        assertEquals("download.bin", LinkPolicy.safeDownloadName("..."));
        assertEquals("原图.png", LinkPolicy.safeDownloadName("原图.png"));
        assertFalse(LinkPolicy.safeDownloadName("evil\u202eapk.exe").contains("\u202e"));
        assertEquals(180, LinkPolicy.safeDownloadName(new String(new char[200]).replace('\0', 'x')).length());
    }

    private static void rejects(String input) {
        try { LinkPolicy.connection(input); fail("Should reject: " + input); }
        catch (IllegalArgumentException expected) { assertNotNull(expected.getMessage()); }
    }
}
