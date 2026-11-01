package com.kroblus.lanrelay;

import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import java.io.BufferedReader;
import java.io.IOException;
import java.io.InputStreamReader;
import java.net.InetSocketAddress;
import java.net.ServerSocket;
import java.net.Socket;
import java.nio.charset.StandardCharsets;
import java.util.Map;
import java.util.TreeMap;
import java.util.concurrent.atomic.AtomicInteger;
import static org.junit.Assert.*;

public class DownloadPreflightTest {
    private ServerSocket server;
    private Thread serverThread;
    private volatile Throwable serverFailure;
    private String origin;
    private final AtomicInteger fileRequests = new AtomicInteger();
    private final AtomicInteger redirectDestinationRequests = new AtomicInteger();

    @Before public void start() throws Exception {
        // Standard java.net only: AGP's unit-test compiler does not expose jdk.httpserver.
        server = new ServerSocket();
        server.bind(new InetSocketAddress("127.0.0.1", 0));
        origin = "http://127.0.0.1:" + server.getLocalPort();
        serverThread = new Thread(() -> {
            while (!server.isClosed()) {
                try (Socket socket = server.accept()) { serve(socket); }
                catch (IOException error) {
                    if (!server.isClosed()) {
                        serverFailure = error;
                        try { server.close(); } catch (IOException ignored) { }
                    }
                    return;
                }
            }
        }, "download-preflight-test-http");
        serverThread.setDaemon(true);
        serverThread.start();
    }

    @After public void stop() throws Exception {
        if (server != null) server.close();
        if (serverThread != null) {
            serverThread.join(3000);
            assertFalse("HTTP test server did not stop", serverThread.isAlive());
        }
        if (serverFailure != null) throw new AssertionError("HTTP test server failed", serverFailure);
    }

    private void serve(Socket socket) throws IOException {
        socket.setSoTimeout(2000);
        BufferedReader reader = new BufferedReader(new InputStreamReader(socket.getInputStream(), StandardCharsets.ISO_8859_1));
        String request = reader.readLine();
        if (request == null) throw new IOException("Missing HTTP request line");
        String[] parts = request.split(" ", 3);
        if (parts.length != 3) throw new IOException("Malformed HTTP request line");
        Map<String, String> headers = new TreeMap<>(String.CASE_INSENSITIVE_ORDER);
        for (int count = 0; ; count++) {
            String line = reader.readLine();
            if (line == null || count > 64) throw new IOException("Incomplete or oversized HTTP headers");
            if (line.isEmpty()) break;
            int separator = line.indexOf(':');
            if (separator <= 0) throw new IOException("Malformed HTTP header");
            headers.put(line.substring(0, separator).trim(), line.substring(separator + 1).trim());
        }
        String status = "200 OK";
        String responseHeaders = "";
        switch (parts[1]) {
            case "/api/files/good":
                fileRequests.incrementAndGet();
                if (!"HEAD".equals(parts[0]) || !"relay_session=synthetic-test".equals(headers.get("Cookie"))) status = "401 Unauthorized";
                else responseHeaders = "Content-Disposition: attachment; filename=\"photo.png\"\r\nContent-Type: image/png\r\n";
                break;
            case "/api/files/revoked":
                status = "401 Unauthorized";
                responseHeaders = "Content-Type: application/json\r\n";
                break;
            case "/api/files/login":
                responseHeaders = "Content-Type: text/html\r\n";
                break;
            case "/api/files/redirect":
                status = "302 Found";
                responseHeaders = "Location: " + origin + "/api/files/target\r\n";
                break;
            case "/api/files/target":
                redirectDestinationRequests.incrementAndGet();
                break;
            case "/api/files/data":
                responseHeaders = "Content-Disposition: attachment; filename=\"data.json\"\r\nContent-Type: application/json; charset=utf-8\r\n";
                break;
            case "/api/feedback/feedback1/export":
                responseHeaders = "Content-Disposition: attachment; filename=\"feedback.md\"\r\nContent-Type: text/markdown; charset=utf-8\r\n";
                break;
            default:
                status = "404 Not Found";
        }
        String response = "HTTP/1.1 " + status + "\r\n" + responseHeaders + "Content-Length: 0\r\nConnection: close\r\n\r\n";
        socket.getOutputStream().write(response.getBytes(StandardCharsets.ISO_8859_1));
        socket.getOutputStream().flush();
    }

    @Test public void sendsOnlyHeadAndCookieToApprovedFileEndpoint() throws Exception {
        DownloadPreflight.Metadata metadata = inspect("/api/files/good");
        assertEquals("image/png", metadata.mime);
        assertEquals(1, fileRequests.get());
    }

    @Test public void revokedJsonResponseIsNotAcceptedAsApkDownload() { denied("/api/files/revoked", "连接已过期"); }
    @Test public void successfulLoginHtmlIsNotAcceptedAsAFile() { denied("/api/files/login", "电脑返回的不是文件"); }
    @Test public void redirectsAreNeverFollowedWithTheCookie() {
        denied("/api/files/redirect", "下载地址发生跳转");
        assertEquals(0, redirectDestinationRequests.get());
    }

    @Test public void realJsonAttachmentAndFeedbackMarkdownRemainDownloadable() throws Exception {
        assertEquals("application/json", inspect("/api/files/data").mime);
        assertEquals("text/markdown", inspect("/api/feedback/feedback1/export").mime);
    }

    @Test public void differentOriginAndHeaderInjectionAreRejectedBeforeNetwork() throws Exception {
        try { DownloadPreflight.inspect("http://10.0.0.1:4318", origin + "/api/files/good", "relay_session=synthetic-test", "test"); fail(); }
        catch (IOException expected) { }
        try { DownloadPreflight.inspect(origin, origin + "/api/files/good", "x\r\nInjected: true", "test"); fail(); }
        catch (IOException expected) { }
        assertEquals(0, fileRequests.get());
    }

    private DownloadPreflight.Metadata inspect(String path) throws IOException {
        return DownloadPreflight.inspect(origin, origin + path, "relay_session=synthetic-test", "LANRelay-test");
    }
    private void denied(String path, String expectedReason) {
        try { inspect(path); fail("Should reject " + path); }
        catch (IOException expected) { assertTrue(expected.getMessage(), expected.getMessage().startsWith(expectedReason)); }
    }
}
