package com.kroblus.lanrelay;

import com.sun.net.httpserver.HttpServer;
import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import java.io.IOException;
import java.net.InetSocketAddress;
import java.util.concurrent.atomic.AtomicInteger;
import static org.junit.Assert.*;

public class DownloadPreflightTest {
    private HttpServer server;
    private String origin;
    private final AtomicInteger fileRequests = new AtomicInteger();
    private final AtomicInteger redirectDestinationRequests = new AtomicInteger();

    @Before public void start() throws Exception {
        server = HttpServer.create(new InetSocketAddress("127.0.0.1", 0), 0);
        origin = "http://127.0.0.1:" + server.getAddress().getPort();
        server.createContext("/api/files/good", exchange -> {
            fileRequests.incrementAndGet();
            if (!"HEAD".equals(exchange.getRequestMethod()) || !"relay_session=synthetic-test".equals(exchange.getRequestHeaders().getFirst("Cookie"))) {
                exchange.sendResponseHeaders(401, -1);
            } else {
                exchange.getResponseHeaders().add("Content-Disposition", "attachment; filename=\"photo.png\"");
                exchange.getResponseHeaders().add("Content-Type", "image/png");
                exchange.sendResponseHeaders(200, -1);
            }
            exchange.close();
        });
        server.createContext("/api/files/revoked", exchange -> {
            exchange.getResponseHeaders().add("Content-Type", "application/json");
            exchange.sendResponseHeaders(401, -1); exchange.close();
        });
        server.createContext("/api/files/login", exchange -> {
            exchange.getResponseHeaders().add("Content-Type", "text/html");
            exchange.sendResponseHeaders(200, -1); exchange.close();
        });
        server.createContext("/api/files/redirect", exchange -> {
            exchange.getResponseHeaders().add("Location", origin + "/api/files/target");
            exchange.sendResponseHeaders(302, -1); exchange.close();
        });
        server.createContext("/api/files/target", exchange -> {
            redirectDestinationRequests.incrementAndGet();
            exchange.sendResponseHeaders(200, -1); exchange.close();
        });
        server.createContext("/api/files/data", exchange -> {
            exchange.getResponseHeaders().add("Content-Disposition", "attachment; filename=\"data.json\"");
            exchange.getResponseHeaders().add("Content-Type", "application/json; charset=utf-8");
            exchange.sendResponseHeaders(200, -1); exchange.close();
        });
        server.createContext("/api/feedback/feedback1/export", exchange -> {
            exchange.getResponseHeaders().add("Content-Disposition", "attachment; filename=\"feedback.md\"");
            exchange.getResponseHeaders().add("Content-Type", "text/markdown; charset=utf-8");
            exchange.sendResponseHeaders(200, -1); exchange.close();
        });
        server.start();
    }

    @After public void stop() { if (server != null) server.stop(0); }

    @Test public void sendsOnlyHeadAndCookieToApprovedFileEndpoint() throws Exception {
        DownloadPreflight.Metadata metadata = inspect("/api/files/good");
        assertEquals("image/png", metadata.mime);
        assertEquals(1, fileRequests.get());
    }

    @Test public void revokedJsonResponseIsNotAcceptedAsApkDownload() { denied("/api/files/revoked"); }
    @Test public void successfulLoginHtmlIsNotAcceptedAsAFile() { denied("/api/files/login"); }
    @Test public void redirectsAreNeverFollowedWithTheCookie() {
        denied("/api/files/redirect");
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
    private void denied(String path) {
        try { inspect(path); fail("Should reject " + path); }
        catch (IOException expected) { assertNotNull(expected.getMessage()); }
    }
}
