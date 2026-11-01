package com.kroblus.lanrelay;

import android.app.AlertDialog;
import android.app.DownloadManager;
import android.content.ActivityNotFoundException;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Context;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.pm.PackageManager;
import android.content.res.ColorStateList;
import android.graphics.Color;
import android.graphics.Typeface;
import android.net.Uri;
import android.net.http.SslError;
import android.os.Build;
import android.os.Bundle;
import android.os.Environment;
import android.provider.Settings;
import android.text.InputType;
import android.text.TextUtils;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.view.WindowInsets;
import android.webkit.CookieManager;
import android.webkit.GeolocationPermissions;
import android.webkit.PermissionRequest;
import android.webkit.SslErrorHandler;
import android.webkit.URLUtil;
import android.webkit.ValueCallback;
import android.webkit.WebChromeClient;
import android.webkit.WebResourceError;
import android.webkit.WebResourceRequest;
import android.webkit.WebResourceResponse;
import android.webkit.WebSettings;
import android.webkit.WebView;
import android.webkit.WebViewClient;
import android.widget.Button;
import android.widget.EditText;
import android.widget.LinearLayout;
import android.widget.ProgressBar;
import android.widget.ScrollView;
import android.widget.TextView;
import android.widget.Toast;

import androidx.activity.ComponentActivity;
import androidx.activity.OnBackPressedCallback;
import androidx.activity.result.ActivityResultLauncher;
import androidx.activity.result.contract.ActivityResultContracts;

import com.journeyapps.barcodescanner.ScanContract;
import com.journeyapps.barcodescanner.ScanOptions;

import java.io.ByteArrayInputStream;
import java.net.URLDecoder;
import java.util.ArrayList;
import java.util.Collections;
import java.util.Objects;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

public final class MainActivity extends ComponentActivity {
    private static final int PAPER = Color.rgb(248, 247, 243);
    private static final int INK = Color.rgb(32, 32, 32);
    private static final int MUTED = Color.rgb(106, 108, 104);
    private static final int CORAL = Color.rgb(233, 116, 87);
    private static final String LOCAL_NETWORK_PERMISSION = "android.permission.ACCESS_LOCAL_NETWORK";
    private final ExecutorService downloads = Executors.newSingleThreadExecutor();

    private SharedPreferences preferences;
    private LinearLayout root;
    private ScrollView connectionScreen;
    private LinearLayout errorPanel;
    private TextView errorText;
    private TextView serverLabel;
    private TextView shareHint;
    private EditText addressInput;
    private ProgressBar loading;
    private WebView webView;
    private volatile String approvedOrigin;
    private LinkPolicy.Endpoint pendingEndpoint;
    private ValueCallback<Uri[]> fileCallback;
    private String chooserOrigin;
    private ArrayList<Uri> sharedFiles = new ArrayList<>();
    private String sharedText;

    private final ActivityResultLauncher<ScanOptions> scanner = registerForActivityResult(new ScanContract(), result -> {
        if (result.getContents() != null) connectInput(result.getContents());
        else toast("扫码已取消；也可以粘贴电脑的连接链接。");
    });
    private final ActivityResultLauncher<String> localNetworkPermission = registerForActivityResult(
            new ActivityResultContracts.RequestPermission(), granted -> {
                if (granted && pendingEndpoint != null) {
                    LinkPolicy.Endpoint endpoint = pendingEndpoint;
                    pendingEndpoint = null;
                    openEndpoint(endpoint);
                } else if (!granted) {
                    showConnection();
                    showError("需要允许访问局域网，才能连接你的电脑。你可以稍后重试，或在系统设置中修改权限。");
                }
            });
    private final ActivityResultLauncher<Intent> filePicker = registerForActivityResult(
            new ActivityResultContracts.StartActivityForResult(), result -> {
                if (fileCallback == null) return;
                if (result.getResultCode() != RESULT_OK || result.getData() == null
                        || !Objects.equals(chooserOrigin, approvedOrigin)) {
                    completeFileChoice(null);
                    return;
                }
                try {
                    ArrayList<Uri> uris = readUris(result.getData());
                    completeFileChoice(validateFiles(uris) ? uris.toArray(new Uri[0]) : null);
                } catch (RuntimeException error) {
                    completeFileChoice(null);
                    toast("文件选择结果无法读取，请使用系统文件选择器重试。");
                }
            });

    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        preferences = getSharedPreferences("connection", MODE_PRIVATE);
        buildInterface();
        configureWebView();
        getOnBackPressedDispatcher().addCallback(this, new OnBackPressedCallback(true) {
            @Override public void handleOnBackPressed() {
                if (webView.getVisibility() == View.VISIBLE && webView.canGoBack()) webView.goBack();
                else if (webView.getVisibility() == View.VISIBLE) showConnection();
                else finish();
            }
        });
        showConnection();
        if (!handleIncoming(getIntent())) openRememberedComputer();
    }

    @Override protected void onNewIntent(Intent intent) {
        super.onNewIntent(intent);
        setIntent(intent);
        handleIncoming(intent);
    }

    private void buildInterface() {
        root = new LinearLayout(this);
        root.setOrientation(LinearLayout.VERTICAL);
        root.setBackgroundColor(PAPER);
        root.setImportantForAutofill(View.IMPORTANT_FOR_AUTOFILL_NO_EXCLUDE_DESCENDANTS);
        root.setOnApplyWindowInsetsListener((view, insets) -> {
            if (Build.VERSION.SDK_INT >= 30) {
                android.graphics.Insets safe = insets.getInsets(WindowInsets.Type.systemBars() | WindowInsets.Type.ime());
                view.setPadding(safe.left, safe.top, safe.right, safe.bottom);
            } else view.setPadding(insets.getSystemWindowInsetLeft(), insets.getSystemWindowInsetTop(),
                    insets.getSystemWindowInsetRight(), insets.getSystemWindowInsetBottom());
            return insets;
        });
        LinearLayout bar = new LinearLayout(this);
        bar.setGravity(Gravity.CENTER_VERTICAL);
        bar.setPadding(dp(16), dp(5), dp(8), dp(5));
        TextView brand = text("↗ 随手传", 20, INK);
        brand.setTypeface(Typeface.DEFAULT, Typeface.BOLD);
        bar.addView(brand, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1));
        Button connect = button("连接", false);
        connect.setOnClickListener(view -> connectionMenu());
        bar.addView(connect);
        Button history = button("下载", false);
        history.setOnClickListener(view -> openDownloads());
        bar.addView(history);
        root.addView(bar);

        serverLabel = text("手机和电脑，在这里接着做", 12, MUTED);
        serverLabel.setPadding(dp(18), 0, dp(18), dp(8));
        serverLabel.setSingleLine(true);
        serverLabel.setEllipsize(TextUtils.TruncateAt.MIDDLE);
        root.addView(serverLabel);

        shareHint = text("", 13, INK);
        shareHint.setPadding(dp(18), dp(10), dp(18), dp(10));
        shareHint.setBackgroundColor(Color.rgb(255, 235, 220));
        shareHint.setVisibility(View.GONE);
        shareHint.setOnClickListener(view -> showSharedContent());
        root.addView(shareHint);

        loading = new ProgressBar(this, null, android.R.attr.progressBarStyleHorizontal);
        loading.setProgressTintList(ColorStateList.valueOf(CORAL));
        loading.setVisibility(View.GONE);
        root.addView(loading, new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, dp(3)));

        connectionScreen = new ScrollView(this);
        connectionScreen.setFillViewport(true);
        LinearLayout content = new LinearLayout(this);
        content.setOrientation(LinearLayout.VERTICAL);
        content.setPadding(dp(24), dp(28), dp(24), dp(20));
        TextView title = text("连上你的电脑", 29, INK);
        title.setTypeface(Typeface.DEFAULT, Typeface.BOLD);
        content.addView(title);
        TextView intro = text("电脑打开随手传，手机与电脑连在可互访的同一网络。", 16, MUTED);
        intro.setPadding(0, dp(12), 0, dp(24));
        content.addView(intro);
        Button scan = button("扫一扫电脑上的配对码", true);
        scan.setOnClickListener(view -> scanner.launch(new ScanOptions()
                .setDesiredBarcodeFormats(ScanOptions.QR_CODE)
                .setPrompt("扫描电脑随手传页面中的配对二维码")
                .setBeepEnabled(false).setOrientationLocked(false).setBarcodeImageEnabled(false)));
        content.addView(scan, fullWidth());

        TextView alternative = text("也可以粘贴连接链接，或输入电脑地址", 14, MUTED);
        alternative.setPadding(0, dp(24), 0, dp(8));
        content.addView(alternative);
        addressInput = new EditText(this);
        addressInput.setHint("http://192.168.1.20:4318");
        addressInput.setTextSize(16);
        addressInput.setSingleLine(true);
        addressInput.setInputType(InputType.TYPE_CLASS_TEXT | InputType.TYPE_TEXT_VARIATION_URI);
        addressInput.setSaveEnabled(false); // Never save a one-time pair token in Activity state.
        addressInput.setImportantForAutofill(View.IMPORTANT_FOR_AUTOFILL_NO);
        addressInput.setText(preferences.getString("origin", ""));
        content.addView(addressInput, fullWidth());

        LinearLayout actions = new LinearLayout(this);
        Button paste = button("粘贴链接", false);
        paste.setOnClickListener(view -> pasteAddress());
        actions.addView(paste, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1));
        Button go = button("连接电脑", true);
        go.setOnClickListener(view -> connectInput(addressInput.getText().toString()));
        actions.addView(go, new LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1));
        content.addView(actions, fullWidth());

        TextView note = text("只记住电脑地址，不保存一次性连接码。第一次连接仍需电脑批准。\n\nHTTP 用于自己的可信局域网，内容不加密。上传时请保持 App 在前台；中断后可重新选择原文件续传。", 14, MUTED);
        note.setPadding(0, dp(22), 0, dp(16));
        note.setLineSpacing(dp(4), 1);
        content.addView(note);
        Button settings = button("局域网权限与系统设置", false);
        settings.setOnClickListener(view -> startActivity(new Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS,
                Uri.parse("package:" + getPackageName()))));
        content.addView(settings, fullWidth());
        connectionScreen.addView(content);
        root.addView(connectionScreen, new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 0, 1));

        errorPanel = new LinearLayout(this);
        errorPanel.setOrientation(LinearLayout.VERTICAL);
        errorPanel.setPadding(dp(20), dp(12), dp(20), dp(12));
        errorText = text("", 14, INK);
        errorPanel.addView(errorText);
        Button retry = button("重试连接", false);
        retry.setOnClickListener(view -> {
            if (approvedOrigin != null) connectInput(approvedOrigin);
            else connectInput(addressInput.getText().toString());
        });
        errorPanel.addView(retry);
        errorPanel.setVisibility(View.GONE);
        root.addView(errorPanel);

        webView = new WebView(this);
        webView.setBackgroundColor(PAPER);
        webView.setVisibility(View.GONE);
        root.addView(webView, new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 0, 1));
        setContentView(root);
        root.requestApplyInsets();
    }

    private void configureWebView() {
        WebView.setWebContentsDebuggingEnabled(false);
        WebSettings settings = webView.getSettings();
        settings.setJavaScriptEnabled(true); // Only the explicitly approved, same-origin LAN app is rendered.
        settings.setDomStorageEnabled(true);
        settings.setAllowFileAccess(false);
        settings.setAllowContentAccess(true); // SAF-selected content:// documents are the only upload source.
        settings.setAllowFileAccessFromFileURLs(false);
        settings.setAllowUniversalAccessFromFileURLs(false);
        settings.setMixedContentMode(WebSettings.MIXED_CONTENT_NEVER_ALLOW);
        settings.setJavaScriptCanOpenWindowsAutomatically(false);
        settings.setSupportMultipleWindows(false);
        settings.setMediaPlaybackRequiresUserGesture(true);
        settings.setGeolocationEnabled(false);
        settings.setSafeBrowsingEnabled(true);
        CookieManager.getInstance().setAcceptCookie(true);
        CookieManager.getInstance().setAcceptThirdPartyCookies(webView, false);
        webView.setWebViewClient(new WebViewClient() {
            @Override public boolean shouldOverrideUrlLoading(WebView view, WebResourceRequest request) {
                String url = request.getUrl().toString();
                if (LinkPolicy.sameOrigin(approvedOrigin, url)) return false;
                if (request.isForMainFrame() && request.hasGesture()) openExternalLink(url);
                return true;
            }
            @Override public WebResourceResponse shouldInterceptRequest(WebView view, WebResourceRequest request) {
                String url = request.getUrl().toString();
                if (LinkPolicy.sameOrigin(approvedOrigin, url) || url.startsWith("blob:") || url.startsWith("data:image/")) return null;
                return new WebResourceResponse("text/plain", "UTF-8", 403, "Blocked",
                        Collections.singletonMap("Cache-Control", "no-store"), new ByteArrayInputStream(new byte[0]));
            }
            @Override public void onPageStarted(WebView view, String url, android.graphics.Bitmap icon) {
                if (!LinkPolicy.sameOrigin(approvedOrigin, url)) { view.stopLoading(); return; }
                loading.setVisibility(View.VISIBLE);
                errorPanel.setVisibility(View.GONE);
            }
            @Override public void onPageFinished(WebView view, String url) {
                loading.setVisibility(View.GONE);
                CookieManager.getInstance().flush();
            }
            @Override public void onReceivedError(WebView view, WebResourceRequest request, WebResourceError error) {
                if (request.isForMainFrame()) showError("没有连上电脑。请确认电脑应用仍在运行、两端网络可互访，并检查局域网权限。");
            }
            @Override public void onReceivedHttpError(WebView view, WebResourceRequest request, WebResourceResponse response) {
                if (request.isForMainFrame()) showError("电脑返回了 HTTP " + response.getStatusCode() + "。请检查连接地址，必要时重新扫码配对。");
            }
            @Override public void onReceivedSslError(WebView view, SslErrorHandler handler, SslError error) {
                handler.cancel();
                showError("HTTPS 证书未通过系统验证，已停止连接。请配置有效证书；App 不会忽略证书错误。");
            }
        });
        webView.setWebChromeClient(new WebChromeClient() {
            @Override public void onProgressChanged(WebView view, int progress) {
                loading.setProgress(progress);
                if (progress >= 100) loading.setVisibility(View.GONE);
            }
            @Override public boolean onShowFileChooser(WebView view, ValueCallback<Uri[]> callback, FileChooserParams parameters) {
                if (!LinkPolicy.sameOrigin(approvedOrigin, view.getUrl())) return false;
                if (parameters.getMode() != FileChooserParams.MODE_OPEN && parameters.getMode() != FileChooserParams.MODE_OPEN_MULTIPLE) {
                    toast("请选择文件；此版本不提供整目录访问。");
                    return false;
                }
                completeFileChoice(null);
                fileCallback = callback;
                chooserOrigin = approvedOrigin;
                if (!sharedFiles.isEmpty()) {
                    new AlertDialog.Builder(MainActivity.this).setTitle("使用分享来的文件？")
                            .setMessage("共有 " + sharedFiles.size() + " 个文件，将由当前电脑的随手传网页接收。")
                            .setPositiveButton("使用这些文件", (dialog, which) -> {
                                ArrayList<Uri> chosen = sharedFiles;
                                sharedFiles = new ArrayList<>();
                                updateShareHint();
                                if (parameters.getMode() == FileChooserParams.MODE_OPEN && chosen.size() != 1) {
                                    toast("这次需要选择一个原文件，请重新选择。");
                                    completeFileChoice(null);
                                } else completeFileChoice(validateFiles(chosen) ? chosen.toArray(new Uri[0]) : null);
                            })
                            .setNegativeButton("重新选择", (dialog, which) -> {
                                sharedFiles.clear(); updateShareHint(); launchFilePicker(parameters);
                            })
                            .setOnCancelListener(dialog -> completeFileChoice(null)).show();
                } else launchFilePicker(parameters);
                return true;
            }
            @Override public void onPermissionRequest(PermissionRequest request) { request.deny(); }
            @Override public void onGeolocationPermissionsShowPrompt(String origin, GeolocationPermissions.Callback callback) {
                callback.invoke(origin, false, false);
            }
        });
        webView.setDownloadListener(this::startDownload);
        // Intentionally no addJavascriptInterface, bridge, native command endpoint or file provider.
    }

    private void connectInput(String input) {
        final LinkPolicy.Endpoint endpoint;
        try { endpoint = LinkPolicy.connection(input); }
        catch (IllegalArgumentException error) { showError(error.getMessage()); return; }
        if (!endpoint.origin.equals(preferences.getString("origin", ""))) {
            new AlertDialog.Builder(this).setTitle("连接这台电脑")
                    .setMessage(endpoint.origin + "\n\n请确认这是你电脑显示的地址。首次配对还需在电脑上批准。"
                            + (endpoint.origin.startsWith("http:") ? "\n当前 HTTP 传输不加密。" : ""))
                    .setPositiveButton("连接", (dialog, which) -> requestLocalNetwork(endpoint))
                    .setNegativeButton("取消", null).show();
        } else requestLocalNetwork(endpoint);
    }

    private void requestLocalNetwork(LinkPolicy.Endpoint endpoint) {
        if (Build.VERSION.SDK_INT >= 37 && checkSelfPermission(LOCAL_NETWORK_PERMISSION) != PackageManager.PERMISSION_GRANTED) {
            pendingEndpoint = endpoint;
            new AlertDialog.Builder(this).setTitle("允许连接局域网电脑")
                    .setMessage("随手传需要访问你选择的电脑，才能传文件和文字。不会扫描整个网络或读取 Wi-Fi 名称。")
                    .setPositiveButton("继续", (dialog, which) -> localNetworkPermission.launch(LOCAL_NETWORK_PERMISSION))
                    .setNegativeButton("暂不连接", (dialog, which) -> pendingEndpoint = null).show();
        } else openEndpoint(endpoint);
    }

    private void openEndpoint(LinkPolicy.Endpoint endpoint) {
        completeFileChoice(null);
        approvedOrigin = endpoint.origin;
        preferences.edit().putString("origin", endpoint.origin).apply();
        addressInput.setText(endpoint.origin); // Pair token is never persisted or put into the visible address label.
        serverLabel.setText(endpoint.origin);
        connectionScreen.setVisibility(View.GONE);
        errorPanel.setVisibility(View.GONE);
        webView.setVisibility(View.VISIBLE);
        webView.loadUrl(endpoint.url);
    }

    private void openRememberedComputer() {
        String origin = preferences.getString("origin", null);
        if (origin == null) return;
        try { requestLocalNetwork(LinkPolicy.connection(origin)); }
        catch (IllegalArgumentException error) { showConnection(); }
    }

    private boolean handleIncoming(Intent intent) {
        if (intent == null) return false;
        String action = intent.getAction();
        if (Intent.ACTION_VIEW.equals(action) && intent.getData() != null) {
            String link = intent.getData().toString();
            intent.setData(null);
            connectInput(link);
            return true;
        }
        if (!Intent.ACTION_SEND.equals(action) && !Intent.ACTION_SEND_MULTIPLE.equals(action)) return false;
        ArrayList<Uri> incoming;
        CharSequence text;
        try {
            incoming = readUris(intent);
            text = intent.getCharSequenceExtra(Intent.EXTRA_TEXT);
        } catch (RuntimeException error) {
            toast("无法读取这次分享，请从原应用重新分享。");
            return true;
        }
        if (!incoming.isEmpty() && validateFiles(incoming)) sharedFiles = incoming;
        intent.removeExtra(Intent.EXTRA_TEXT);
        intent.removeExtra(Intent.EXTRA_STREAM);
        intent.setClipData(null);
        if (text != null && text.length() > 0 && text.length() <= 200000) {
            String value = text.toString().trim();
            try {
                LinkPolicy.Endpoint endpoint = LinkPolicy.connection(value);
                if (endpoint.pairedLink || value.startsWith("lanrelay:")) {
                    connectInput(value); updateShareHint(); return true;
                }
            } catch (IllegalArgumentException ignored) { /* Ordinary shared text stays text, never a navigation. */ }
            sharedText = text.toString();
        }
        updateShareHint();
        if (approvedOrigin == null) openRememberedComputer();
        if (sharedFiles.isEmpty() && sharedText == null) toast("没有可读取的分享内容，请从文件管理器重新分享。");
        return true;
    }

    private ArrayList<Uri> readUris(Intent intent) {
        ArrayList<Uri> result = new ArrayList<>();
        if (Intent.ACTION_SEND_MULTIPLE.equals(intent.getAction())) {
            ArrayList<?> streams = intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM);
            if (streams != null && streams.size() > 64) throw new IllegalArgumentException("Too many shared files");
            if (streams != null) for (Object stream : streams) {
                if (!(stream instanceof Uri)) throw new IllegalArgumentException("Invalid shared URI");
                result.add((Uri) stream);
            }
        } else {
            Object stream = intent.getParcelableExtra(Intent.EXTRA_STREAM);
            if (stream instanceof Uri) result.add((Uri) stream);
            else if (stream != null) throw new IllegalArgumentException("Invalid shared URI");
        }
        if (result.isEmpty() && intent.getClipData() != null) {
            ClipData clips = intent.getClipData();
            if (clips.getItemCount() > 64) throw new IllegalArgumentException("Too many shared files");
            for (int i = 0; i < clips.getItemCount(); i++) {
                Uri uri = clips.getItemAt(i).getUri();
                if (uri != null) result.add(uri);
            }
        }
        if (result.isEmpty() && intent.getData() != null && "content".equals(intent.getData().getScheme())) result.add(intent.getData());
        return new ArrayList<>(new java.util.LinkedHashSet<>(result));
    }

    private boolean validateFiles(ArrayList<Uri> files) {
        if (files.isEmpty() || files.size() > 64) { toast("每次请选择 1 至 64 个文件。"); return false; }
        for (Uri uri : files) {
            String authority = uri == null ? null : uri.getAuthority();
            if (uri == null || !"content".equals(uri.getScheme()) || authority == null
                    || authority.startsWith(getPackageName()) || authority.startsWith("com.kroblus.lanrelay")
                    || checkUriPermission(uri, android.os.Process.myPid(), android.os.Process.myUid(), Intent.FLAG_GRANT_READ_URI_PERMISSION)
                        != PackageManager.PERMISSION_GRANTED) {
                toast("没有读取所选文件的权限，请使用系统文件选择器重新选择。");
                return false;
            }
        }
        return true;
    }

    private void launchFilePicker(WebChromeClient.FileChooserParams parameters) {
        Intent choose = new Intent(Intent.ACTION_OPEN_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE);
        ArrayList<String> types = new ArrayList<>();
        for (String type : parameters.getAcceptTypes()) {
            if (type != null && type.matches("[A-Za-z0-9.+*-]+/[A-Za-z0-9.+*-]+") && !types.contains(type)) types.add(type);
        }
        choose.setType(types.size() == 1 ? types.get(0) : "*/*");
        if (types.size() > 1) choose.putExtra(Intent.EXTRA_MIME_TYPES, types.toArray(new String[0]));
        choose.putExtra(Intent.EXTRA_ALLOW_MULTIPLE, parameters.getMode() == WebChromeClient.FileChooserParams.MODE_OPEN_MULTIPLE);
        choose.addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
        try { filePicker.launch(choose); }
        catch (ActivityNotFoundException error) { completeFileChoice(null); toast("系统没有可用的文件选择器。"); }
    }

    private void completeFileChoice(Uri[] values) {
        ValueCallback<Uri[]> callback = fileCallback;
        fileCallback = null;
        chooserOrigin = null;
        if (callback != null) callback.onReceiveValue(values);
    }

    private void startDownload(String url, String userAgent, String disposition, String mime, long length) {
        final String origin = approvedOrigin;
        if (!LinkPolicy.authenticatedDownload(origin, url)) { toast("这里只能下载当前电脑工作区里的文件。"); return; }
        if (Build.VERSION.SDK_INT >= 37 && checkSelfPermission(LOCAL_NETWORK_PERMISSION) != PackageManager.PERMISSION_GRANTED) {
            showError("局域网权限已关闭，请先允许连接，再重新下载。"); return;
        }
        final String cookie = CookieManager.getInstance().getCookie(url);
        toast("正在向电脑确认文件…");
        downloads.execute(() -> {
            try {
                DownloadPreflight.Metadata metadata = DownloadPreflight.inspect(origin, url, cookie, userAgent);
                runOnUiThread(() -> {
                    if (isDestroyed() || !Objects.equals(origin, approvedOrigin)) return;
                    try {
                        String filename = filename(url, metadata.disposition, metadata.mime);
                        DownloadManager.Request request = new DownloadManager.Request(Uri.parse(url));
                        request.addRequestHeader("Cookie", cookie);
                        if (userAgent != null) request.addRequestHeader("User-Agent", userAgent);
                        request.setMimeType(metadata.mime).setTitle(filename)
                                .setDescription("来自你的随手传电脑")
                                .setNotificationVisibility(DownloadManager.Request.VISIBILITY_VISIBLE_NOTIFY_COMPLETED)
                                .setAllowedOverRoaming(false)
                                .setDestinationInExternalPublicDir(Environment.DIRECTORY_DOWNLOADS,
                                        "LANRelay/" + System.currentTimeMillis() + "-" + filename);
                        ((DownloadManager) getSystemService(DOWNLOAD_SERVICE)).enqueue(request);
                        toast("已交给系统下载。请点右上角“下载”查看实际结果；这不代表 APK 已安装。");
                    } catch (RuntimeException error) { showError("系统未接受下载请求，请检查可用空间和下载管理器。"); }
                });
            } catch (java.io.IOException error) {
                String message = error.getMessage();
                runOnUiThread(() -> {
                    if (!isDestroyed()) showError(message != null && message.startsWith("电脑") || message != null && message.startsWith("连接")
                            || message != null && message.startsWith("下载") || message != null && message.startsWith("只允许")
                            ? message : "无法向电脑确认文件，请检查连接后重试。");
                });
            }
        });
    }

    private static String filename(String url, String disposition, String mime) {
        String value = URLUtil.guessFileName(url, disposition, mime);
        java.util.regex.Matcher utf8 = java.util.regex.Pattern.compile("(?i)filename\\*=UTF-8''([^;\\r\\n]+)").matcher(disposition);
        if (utf8.find()) {
            try { value = URLDecoder.decode(utf8.group(1).trim().replace("+", "%2B"), "UTF-8"); }
            catch (Exception ignored) { /* Keep the platform's fallback. */ }
        }
        return LinkPolicy.safeDownloadName(value);
    }

    private void openExternalLink(String url) {
        if (url.startsWith("lanrelay:")) { connectInput(url); return; }
        Uri uri = Uri.parse(url);
        if (!("http".equals(uri.getScheme()) || "https".equals(uri.getScheme())) || uri.getHost() == null) {
            toast("此链接不能在随手传中打开。"); return;
        }
        new AlertDialog.Builder(this).setTitle("在浏览器中打开")
                .setMessage(uri.getHost() + "\n\n将离开随手传。浏览器使用自己的登录状态。")
                .setPositiveButton("打开浏览器", (dialog, which) -> {
                    try { startActivity(new Intent(Intent.ACTION_VIEW, uri)); }
                    catch (ActivityNotFoundException error) { toast("没有可用的浏览器。"); }
                }).setNegativeButton("取消", null).show();
    }

    private void connectionMenu() {
        new AlertDialog.Builder(this).setTitle("连接与使用")
                .setItems(new String[]{"扫一扫重新配对", "更换电脑地址", "刷新当前网页", "分享内容说明", "权限与系统设置"},
                        (dialog, which) -> {
                            if (which == 0) scanner.launch(new ScanOptions().setDesiredBarcodeFormats(ScanOptions.QR_CODE)
                                    .setPrompt("扫描电脑中的随手传配对二维码").setBeepEnabled(false).setOrientationLocked(false));
                            else if (which == 1) confirmInterruption(this::showConnection);
                            else if (which == 2) confirmInterruption(() -> { if (approvedOrigin != null) connectInput(approvedOrigin); });
                            else if (which == 3) new AlertDialog.Builder(this).setTitle("从其他 App 分享")
                                    .setMessage("图片、APK、录屏等可通过系统“分享 → 随手传”交给本 App，再点网页中的“文件”确认发送。\n\n分享的文字可点顶部提示复制，再粘贴到网页发送框。文件不压缩、不转码。\n\n浏览器与本 App 的登录相互独立，需要各自配对。")
                                    .setPositiveButton("知道了", null).show();
                            else startActivity(new Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.parse("package:" + getPackageName())));
                        }).show();
    }

    private void confirmInterruption(Runnable action) {
        if (webView.getVisibility() != View.VISIBLE) { action.run(); return; }
        new AlertDialog.Builder(this).setTitle("切换当前页面")
                .setMessage("刷新或更换电脑可能中断当前上传。请先在网页中暂停任务；之后可以重选原文件续传。")
                .setPositiveButton("继续", (dialog, which) -> { webView.stopLoading(); action.run(); })
                .setNegativeButton("取消", null).show();
    }

    private void showConnection() {
        connectionScreen.setVisibility(View.VISIBLE);
        webView.setVisibility(View.GONE);
        loading.setVisibility(View.GONE);
    }

    private void showError(String message) {
        errorText.setText(message);
        errorPanel.setVisibility(View.VISIBLE);
        loading.setVisibility(View.GONE);
    }

    private void pasteAddress() {
        ClipboardManager clipboard = (ClipboardManager) getSystemService(CLIPBOARD_SERVICE);
        ClipData clip = clipboard.getPrimaryClip();
        if (clip == null || clip.getItemCount() == 0) { toast("剪贴板里没有连接链接。"); return; }
        CharSequence text = clip.getItemAt(0).getText();
        if (text == null || text.length() > 4096) { toast("请复制电脑显示的连接链接。"); return; }
        addressInput.setText(text);
    }

    private void updateShareHint() {
        if (sharedText != null) shareHint.setText("收到分享文字 · 点这里复制后发送");
        else if (!sharedFiles.isEmpty()) shareHint.setText("收到 " + sharedFiles.size() + " 个文件 · 点网页“文件”确认发送");
        shareHint.setVisibility(sharedText != null || !sharedFiles.isEmpty() ? View.VISIBLE : View.GONE);
    }

    private void showSharedContent() {
        if (sharedText == null) { toast("连接电脑后，点网页中的“文件”，选择使用分享来的文件。"); return; }
        String value = sharedText;
        String preview = value.length() > 200 ? value.substring(0, 200) + "…" : value;
        new AlertDialog.Builder(this).setTitle("分享来的文字").setMessage(preview)
                .setPositiveButton("复制文字", (dialog, which) -> {
                    ((ClipboardManager) getSystemService(CLIPBOARD_SERVICE)).setPrimaryClip(ClipData.newPlainText("随手传待发送文字", value));
                    sharedText = null; updateShareHint(); toast("已复制。请在网页发送框粘贴并发送。");
                }).setNegativeButton("稍后", null).show();
    }

    private void openDownloads() {
        try { startActivity(new Intent(DownloadManager.ACTION_VIEW_DOWNLOADS)); }
        catch (ActivityNotFoundException error) { toast("请在系统文件管理器的 Download/LANRelay 目录查看下载结果。"); }
    }

    @Override protected void onPause() {
        CookieManager.getInstance().flush();
        webView.onPause();
        super.onPause();
    }
    @Override protected void onResume() {
        super.onResume();
        if (webView != null) webView.onResume();
    }
    @Override protected void onDestroy() {
        completeFileChoice(null);
        downloads.shutdownNow();
        webView.stopLoading();
        webView.destroy();
        super.onDestroy();
    }

    private TextView text(String value, int size, int color) {
        TextView text = new TextView(this); text.setText(value); text.setTextSize(size); text.setTextColor(color); return text;
    }
    private Button button(String label, boolean primary) {
        Button button = new Button(this);
        button.setText(label); button.setTextSize(14); button.setAllCaps(false); button.setMinHeight(dp(48));
        button.setMinWidth(0); button.setMinimumWidth(0);
        button.setPadding(dp(12), dp(6), dp(12), dp(6));
        button.setTextColor(primary ? Color.WHITE : INK);
        button.setBackgroundTintList(ColorStateList.valueOf(primary ? CORAL : Color.rgb(237, 236, 230)));
        return button;
    }
    private LinearLayout.LayoutParams fullWidth() {
        LinearLayout.LayoutParams params = new LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT);
        params.topMargin = dp(5); params.bottomMargin = dp(5); return params;
    }
    private int dp(int value) { return Math.round(value * getResources().getDisplayMetrics().density); }
    private void toast(String message) { Toast.makeText(this, message, Toast.LENGTH_LONG).show(); }
}
