package com.kroblus.lanrelay;

import java.io.IOException;
import java.net.HttpURLConnection;
import java.net.URL;

/** Header-only check; never follows a redirect or stores a response body. */
public final class DownloadPreflight {
    private DownloadPreflight() { }

    public static final class Metadata {
        public final String disposition;
        public final String mime;
        Metadata(String disposition, String mime) { this.disposition = disposition; this.mime = mime; }
    }

    public static Metadata inspect(String origin, String url, String cookie, String userAgent) throws IOException {
        if (!LinkPolicy.authenticatedDownload(origin, url)) throw new IOException("只允许下载当前电脑工作区内的文件。");
        if (cookie == null || cookie.isEmpty()) throw new IOException("连接已失效，请重新配对后下载。");
        if (cookie.indexOf('\r') >= 0 || cookie.indexOf('\n') >= 0) throw new IOException("会话信息无效，请重新配对。");
        HttpURLConnection connection = (HttpURLConnection) new URL(url).openConnection();
        try {
            connection.setRequestMethod("HEAD");
            connection.setInstanceFollowRedirects(false);
            connection.setConnectTimeout(8000);
            connection.setReadTimeout(8000);
            connection.setUseCaches(false);
            connection.setRequestProperty("Cookie", cookie);
            if (userAgent != null && userAgent.indexOf('\r') < 0 && userAgent.indexOf('\n') < 0) {
                connection.setRequestProperty("User-Agent", userAgent);
            }
            int status = connection.getResponseCode();
            if (status == 401 || status == 403) throw new IOException("连接已过期或已被电脑撤销，请重新配对后下载。");
            if (status >= 300 && status < 400) throw new IOException("下载地址发生跳转，已停止。请使用当前电脑的文件入口。");
            if (status != 200) throw new IOException("电脑没有提供可下载的文件（HTTP " + status + "）。");
            String disposition = connection.getHeaderField("Content-Disposition");
            if (disposition == null || !disposition.trim().toLowerCase(java.util.Locale.ROOT).startsWith("attachment;")) {
                throw new IOException("电脑返回的不是文件下载响应，已停止保存。");
            }
            String mime = connection.getContentType();
            if (mime == null || mime.indexOf('\r') >= 0 || mime.indexOf('\n') >= 0) mime = "application/octet-stream";
            return new Metadata(disposition, mime.split(";", 2)[0].trim());
        } finally { connection.disconnect(); }
    }
}
