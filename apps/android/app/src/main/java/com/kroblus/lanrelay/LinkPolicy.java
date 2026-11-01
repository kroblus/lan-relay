package com.kroblus.lanrelay;

import java.net.InetAddress;
import java.net.URI;
import java.net.URISyntaxException;
import java.net.URLDecoder;
import java.util.HashSet;
import java.util.Locale;
import java.util.Set;

/** No Android dependency: this boundary is exercised by local JVM tests. */
public final class LinkPolicy {
    private LinkPolicy() { }

    public static final class Endpoint {
        public final String origin;
        public final String url;
        public final boolean pairedLink;

        private Endpoint(String origin, String url, boolean pairedLink) {
            this.origin = origin;
            this.url = url;
            this.pairedLink = pairedLink;
        }
    }

    public static Endpoint connection(String input) {
        if (input == null || input.trim().isEmpty() || input.length() > 4096) {
            throw invalid("请输入电脑显示的局域网地址，或扫描配对二维码。");
        }
        String value = input.trim();
        if (value.indexOf('\\') >= 0 || value.matches("(?s).*\\p{Cntrl}.*")) {
            throw invalid("连接地址包含无效字符。");
        }
        if (value.toLowerCase(Locale.ROOT).startsWith("lanrelay:")) value = unwrap(value);
        boolean implicitScheme = !value.contains("://");
        if (implicitScheme) value = "http://" + value;
        URI uri = parse(value);
        String scheme = lower(uri.getScheme());
        if (!("http".equals(scheme) || "https".equals(scheme)) || uri.getRawUserInfo() != null
                || uri.getHost() == null || uri.getRawQuery() != null) {
            throw invalid("只接受电脑的 HTTP / HTTPS 局域网连接地址。");
        }
        String host = lower(uri.getHost());
        if (!isLocalHost(host)) {
            throw invalid("请使用电脑的局域网 IP 或本地域名；127.0.0.1 指的是手机自己。");
        }
        String path = uri.getRawPath();
        if (path != null && !path.isEmpty() && !"/".equals(path)) {
            throw invalid("请复制电脑的连接入口，而不是文件或其他页面地址。");
        }
        int port = uri.getPort();
        if (implicitScheme && port == -1) port = 4318;
        if (port == 0 || port < -1 || port > 65535) throw invalid("端口应在 1 至 65535 之间。");
        String origin;
        try { origin = new URI(scheme, null, host, port, null, null, null).toASCIIString(); }
        catch (URISyntaxException error) { throw invalid("无法识别这个连接地址。"); }
        String fragment = normalizeFragment(uri.getRawFragment());
        return new Endpoint(origin, origin + "/" + (fragment.isEmpty() ? "" : "#" + fragment), !fragment.isEmpty());
    }

    private static String unwrap(String input) {
        URI link = parse(input);
        if (!"lanrelay".equalsIgnoreCase(link.getScheme()) || !"connect".equalsIgnoreCase(link.getHost())
                || link.getRawUserInfo() != null || link.getPort() != -1 || link.getRawFragment() != null
                || (link.getRawPath() != null && !link.getRawPath().isEmpty())
                || link.getRawQuery() == null || !link.getRawQuery().startsWith("url=")) {
            throw invalid("无法识别这个 App 连接链接。");
        }
        String raw = link.getRawQuery().substring(4);
        if (raw.indexOf('&') >= 0 || raw.isEmpty()) throw invalid("App 连接链接缺少唯一的电脑地址。");
        String target = decode(raw);
        if (!(target.startsWith("http://") || target.startsWith("https://"))) {
            throw invalid("App 连接链接中必须是电脑的 HTTP / HTTPS 地址。");
        }
        return target;
    }

    private static String normalizeFragment(String fragment) {
        if (fragment == null || fragment.isEmpty()) return "";
        StringBuilder normalized = new StringBuilder();
        Set<String> seen = new HashSet<>();
        for (String entry : fragment.split("&", -1)) {
            String[] parts = entry.split("=", 2);
            if (parts.length != 2) throw invalid("配对链接格式不完整，请重新扫码。");
            String key = decode(parts[0]);
            String value = decode(parts[1]);
            if (!seen.add(key)) throw invalid("配对链接含重复参数，请重新扫码。");
            if ("owner".equalsIgnoreCase(key)) throw invalid("这是电脑管理入口。手机请扫描网页内的配对二维码。");
            if ("pair".equals(key)) {
                if (!value.matches("[A-Za-z0-9_-]{16,256}")) throw invalid("配对信息格式有误，请重新扫码。");
            } else if ("code".equals(key)) {
                if (!value.matches("[0-9]{6}")) throw invalid("连接码必须是六位数字。");
            } else throw invalid("配对链接含不支持的参数，请使用电脑显示的连接入口。");
            if (normalized.length() > 0) normalized.append('&');
            normalized.append(key).append('=').append(value);
        }
        return normalized.toString();
    }

    public static boolean sameOrigin(String approvedOrigin, String url) {
        if (approvedOrigin == null || url == null || url.indexOf('\\') >= 0) return false;
        try {
            URI expected = new URI(approvedOrigin);
            URI actual = new URI(url);
            String scheme = lower(actual.getScheme());
            return ("http".equals(scheme) || "https".equals(scheme)) && actual.getRawUserInfo() == null
                    && actual.getHost() != null && scheme.equals(lower(expected.getScheme()))
                    && lower(actual.getHost()).equals(lower(expected.getHost()))
                    && effectivePort(actual) == effectivePort(expected);
        } catch (URISyntaxException error) { return false; }
    }

    public static boolean authenticatedDownload(String approvedOrigin, String url) {
        if (!sameOrigin(approvedOrigin, url)) return false;
        URI uri = parse(url);
        if (uri.getRawQuery() != null || uri.getRawFragment() != null) return false;
        String path = uri.getRawPath();
        return path != null && (path.matches("/api/files/[A-Za-z0-9_-]{1,128}")
                || path.matches("/api/feedback/[A-Za-z0-9_-]{1,128}/export"));
    }

    public static String safeDownloadName(String filename) {
        String name = filename == null ? "" : filename.replaceAll("[\\\\/:*?\"<>|\\p{Cntrl}\\u202a-\\u202e\\u2066-\\u2069]", "_").trim();
        while (name.startsWith(".")) name = name.substring(1);
        if (name.length() > 180) {
            name = name.substring(0, 180);
            if (Character.isHighSurrogate(name.charAt(name.length() - 1))) name = name.substring(0, name.length() - 1);
        }
        return name.isEmpty() ? "download.bin" : name;
    }

    static boolean isLocalHost(String input) {
        String host = lower(input);
        if (host.startsWith("[") && host.endsWith("]")) host = host.substring(1, host.length() - 1);
        if (host.indexOf(':') >= 0) {
            if (!host.matches("[0-9a-f:]+")) return false;
            try {
                byte[] bytes = InetAddress.getByName(host).getAddress(); // Literal-only; never resolves a DNS name.
                return bytes.length == 16 && (((bytes[0] & 0xfe) == 0xfc)
                        || ((bytes[0] & 0xff) == 0xfe && (bytes[1] & 0xc0) == 0x80));
            } catch (Exception error) { return false; }
        }
        if (host.matches("[0-9.]+")) {
            String[] parts = host.split("\\.", -1);
            if (parts.length != 4) return false;
            int[] octets = new int[4];
            for (int i = 0; i < 4; i++) {
                if (!parts[i].matches("0|[1-9][0-9]{0,2}")) return false;
                octets[i] = Integer.parseInt(parts[i]);
                if (octets[i] > 255) return false;
            }
            return octets[0] == 10 || (octets[0] == 172 && octets[1] >= 16 && octets[1] <= 31)
                    || (octets[0] == 192 && octets[1] == 168) || (octets[0] == 169 && octets[1] == 254)
                    || (octets[0] == 100 && octets[1] >= 64 && octets[1] <= 127);
        }
        if (host.matches("0x[0-9a-f]+") || host.length() > 253 || !host.matches("[a-z0-9](?:[a-z0-9.-]*[a-z0-9])?")) return false;
        if (host.equals("localhost") || host.endsWith(".localhost") || host.contains("..")) return false;
        return host.endsWith(".local") || host.endsWith(".lan") || host.endsWith(".home.arpa")
                || (host.indexOf('.') < 0 && host.matches(".*[a-z].*"));
    }

    private static int effectivePort(URI uri) { return uri.getPort() == -1 ? ("https".equalsIgnoreCase(uri.getScheme()) ? 443 : 80) : uri.getPort(); }
    private static URI parse(String value) { try { return new URI(value); } catch (URISyntaxException error) { throw invalid("无法识别这个连接地址。"); } }
    private static String decode(String value) { try { return URLDecoder.decode(value, "UTF-8"); } catch (Exception error) { throw invalid("连接地址编码不正确。"); } }
    private static String lower(String value) { return value == null ? "" : value.toLowerCase(Locale.ROOT); }
    private static IllegalArgumentException invalid(String message) { return new IllegalArgumentException(message); }
}
