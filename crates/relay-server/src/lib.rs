//! Versioned native-client LAN protocol. Owner actions are intentionally absent.
pub mod discovery;
use axum::{
    body::{Body, Bytes},
    extract::{ConnectInfo, DefaultBodyLimit, Path, Query, State},
    http::{header, HeaderMap, HeaderValue, Method, Request, StatusCode},
    middleware::{self, Next},
    response::{
        sse::{Event, KeepAlive},
        IntoResponse, Response, Sse,
    },
    routing::{get, patch, post},
    Extension, Json, Router,
};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use relay_core::{
    AuthenticatedSession, ClaimRequest, NewDiscoveryPairRequest, NewFeedback, NewPairRequest,
    NewText, NewUpload, PatchItem, Relay, RelayError,
};
use serde::Deserialize;
use std::{
    collections::HashMap,
    convert::Infallible,
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt},
    net::TcpListener,
    sync::{oneshot, Semaphore},
    task::JoinHandle,
};

#[derive(Clone)]
struct AppState {
    relay: Arc<Relay>,
    workers: Arc<Semaphore>,
    requests: Arc<Semaphore>,
    downloads: Arc<Semaphore>,
    pairing_rates: Arc<Mutex<HashMap<IpAddr, (Instant, u32)>>>,
    claim_rates: Arc<Mutex<HashMap<IpAddr, (Instant, u32)>>>,
}

pub struct ServerHandle {
    pub addr: SocketAddr,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<std::io::Result<()>>>,
}

impl ServerHandle {
    pub async fn shutdown(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            // Clients must not indefinitely hold the process alive with event streams.
            let mut task = task;
            if tokio::time::timeout(Duration::from_secs(5), &mut task)
                .await
                .is_err()
            {
                task.abort();
            }
        }
    }
}
impl Drop for ServerHandle {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

pub async fn start(relay: Arc<Relay>, bind: SocketAddr) -> std::io::Result<ServerHandle> {
    let listener = TcpListener::bind(bind).await?;
    let addr = listener.local_addr()?;
    let (stop, stopped) = oneshot::channel();
    let app = router(relay);
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .with_graceful_shutdown(async {
            let _ = stopped.await;
        })
        .await
    });
    Ok(ServerHandle {
        addr,
        stop: Some(stop),
        task: Some(task),
    })
}

pub fn router(relay: Arc<Relay>) -> Router {
    let max_chunk = relay.hello().chunk_size as usize;
    let state = AppState {
        relay,
        workers: Arc::new(Semaphore::new(4)),
        requests: Arc::new(Semaphore::new(32)),
        downloads: Arc::new(Semaphore::new(8)),
        pairing_rates: Arc::new(Mutex::new(HashMap::new())),
        claim_rates: Arc::new(Mutex::new(HashMap::new())),
    };
    Router::new()
        .route("/api/v2/hello", get(hello))
        .route(
            "/api/v2/pair/requests",
            post(pair_request).layer(DefaultBodyLimit::max(4096)),
        )
        .route(
            "/api/v2/pair/discovery",
            post(discovery_pair).layer(DefaultBodyLimit::max(4096)),
        )
        .route(
            "/api/v2/pair/claim",
            post(pair_claim).layer(DefaultBodyLimit::max(4096)),
        )
        .route(
            "/api/v2/pair/cancel",
            post(pair_cancel).layer(DefaultBodyLimit::max(4096)),
        )
        .route("/api/v2/state", get(snapshot))
        .route("/api/v2/items", get(items))
        .route("/api/v2/text", post(text))
        .route("/api/v2/feedback", post(feedback))
        .route("/api/v2/items/{id}", patch(edit_item))
        .route("/api/v2/feedback/{id}/export", get(export_feedback))
        .route("/api/v2/uploads", post(create_upload))
        .route(
            "/api/v2/uploads/{id}",
            get(get_upload).patch(append_upload).delete(cancel_upload),
        )
        .route("/api/v2/uploads/{id}/complete", post(complete_upload))
        .route("/api/v2/files/{id}", get(download))
        .route("/api/v2/events", get(events))
        .fallback(|| async {
            ApiError(RelayError::new(
                404,
                "NOT_FOUND",
                "此地址不是随手传 V2 接口",
            ))
        })
        .layer(DefaultBodyLimit::max(max_chunk.max(128 * 1024)))
        .layer(middleware::from_fn_with_state(state.clone(), boundary))
        .with_state(state)
}

struct ApiError(RelayError);
impl From<RelayError> for ApiError {
    fn from(e: RelayError) -> Self {
        Self(e)
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            StatusCode::from_u16(self.0.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(serde_json::json!({"error":{"code":self.0.code,"message":self.0.message}})),
        )
            .into_response()
    }
}
type ApiResult<T> = Result<T, ApiError>;

async fn boundary(
    State(state): State<AppState>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    // This API is for native clients. An unrelated browser origin must never gain
    // access to pairing or credentials through DNS rebinding / a permissive CORS layer.
    if request.headers().contains_key(header::ORIGIN) {
        return ApiError(RelayError::new(
            403,
            "ORIGIN_REJECTED",
            "请使用原生客户端连接此服务",
        ))
        .into_response();
    }
    let permit = match state.requests.clone().try_acquire_owned() {
        Ok(p) => p,
        Err(_) => {
            return ApiError(RelayError::new(503, "SERVER_BUSY", "服务繁忙，请稍后重试"))
                .into_response()
        }
    };
    let path = request.uri().path();
    if path != "/api/v2/hello" && !path.starts_with("/api/v2/pair/") {
        let token = match bearer(request.headers()) {
            Ok(v) => v,
            Err(e) => return e.into_response(),
        };
        // Authenticate before body allocation, then reuse only within this
        // request. Handlers recheck expiry after reading the body and core
        // operations still check device revocation before accessing data.
        match work(&state, move |r| r.authenticate_session(&token)).await {
            Ok(session) => {
                request.extensions_mut().insert(session);
            }
            Err(e) => return e.into_response(),
        }
    }
    let response = tokio::time::timeout(Duration::from_secs(120), next.run(request)).await;
    drop(permit);
    let mut response = match response {
        Ok(r) => r,
        Err(_) => ApiError(RelayError::new(
            408,
            "REQUEST_TIMEOUT",
            "请求超时，可重新查询任务后重试",
        ))
        .into_response(),
    };
    if (response.status().is_client_error() || response.status().is_server_error())
        && !response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("application/json"))
    {
        response = ApiError(RelayError::new(
            response.status().as_u16(),
            "INVALID_REQUEST",
            "请求格式、字段或大小不正确，请检查客户端协议版本",
        ))
        .into_response();
    }
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    response
        .headers_mut()
        .insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    response
}

async fn work<T, F>(state: &AppState, f: F) -> ApiResult<T>
where
    T: Send + 'static,
    F: FnOnce(&Relay) -> relay_core::Result<T> + Send + 'static,
{
    let permit = state
        .workers
        .clone()
        .acquire_owned()
        .await
        .map_err(|_| ApiError(RelayError::new(503, "SERVER_STOPPING", "服务正在停止")))?;
    let relay = state.relay.clone();
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        f(&relay)
    })
    .await
    .map_err(|_| {
        ApiError(RelayError::new(
            500,
            "WORKER_FAILED",
            "传输处理失败，请重新查询任务状态",
        ))
    })?
    .map_err(Into::into)
}
fn bearer(headers: &HeaderMap) -> ApiResult<String> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .filter(|s| s.len() == 43)
        .map(str::to_owned)
        .ok_or_else(|| {
            ApiError(RelayError::new(
                401,
                "UNAUTHENTICATED",
                "需要有效的设备凭据",
            ))
        })
}

async fn hello(State(state): State<AppState>) -> Json<relay_core::Hello> {
    Json(state.relay.hello())
}
async fn pair_request(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(input): Json<NewPairRequest>,
) -> ApiResult<Json<relay_core::PairTicket>> {
    rate_pairing(&state, peer.ip(), false)?;
    Ok(Json(work(&state, move |r| r.request_pair(input)).await?))
}

fn rate_pairing(state: &AppState, peer: IpAddr, claim: bool) -> ApiResult<()> {
    let mut rates = if claim {
        &state.claim_rates
    } else {
        &state.pairing_rates
    }
    .lock()
    .map_err(|_| ApiError(RelayError::new(503, "SERVER_BUSY", "服务繁忙")))?;
    let now = Instant::now();
    rates.retain(|_, v| now.duration_since(v.0) < Duration::from_secs(60));
    if rates.len() >= 1024 && !rates.contains_key(&peer) {
        return Err(ApiError(RelayError::new(
            429,
            "PAIR_RATE_LIMIT",
            "连接请求过多，请稍后重试",
        )));
    }
    let entry = rates.entry(peer).or_insert((now, 0));
    entry.1 = entry.1.saturating_add(1);
    if entry.1 > if claim { 120 } else { 12 } {
        return Err(ApiError(RelayError::new(
            429,
            "PAIR_RATE_LIMIT",
            "连接请求过于频繁，请一分钟后重试",
        )));
    }
    Ok(())
}

async fn discovery_pair(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(input): Json<NewDiscoveryPairRequest>,
) -> ApiResult<Json<relay_core::PairTicket>> {
    rate_pairing(&state, peer.ip(), false)?;
    let is_local = match peer.ip() {
        IpAddr::V4(ip) => discovery::private_ipv4(ip) || ip.is_loopback(),
        IpAddr::V6(ip) => ip.is_unique_local() || ip.is_unicast_link_local() || ip.is_loopback(),
    };
    if !is_local {
        return Err(ApiError(RelayError::new(
            403,
            "LAN_REQUIRED",
            "请在同一局域网内连接设备",
        )));
    }
    Ok(Json(
        work(&state, move |r| r.request_discovery_pair(input)).await?,
    ))
}
async fn pair_claim(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(input): Json<ClaimRequest>,
) -> ApiResult<Json<relay_core::PairClaim>> {
    rate_pairing(&state, peer.ip(), true)?;
    Ok(Json(work(&state, move |r| r.claim_pair(input)).await?))
}
async fn pair_cancel(
    State(state): State<AppState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(input): Json<ClaimRequest>,
) -> ApiResult<Json<relay_core::PairClaim>> {
    rate_pairing(&state, peer.ip(), true)?;
    Ok(Json(work(&state, move |r| r.cancel_pair(input)).await?))
}
async fn snapshot(
    State(state): State<AppState>,
    Extension(session): Extension<AuthenticatedSession>,
) -> ApiResult<Json<relay_core::Snapshot>> {
    let a = session.actor()?.to_owned();
    Ok(Json(work(&state, move |r| r.snapshot(&a)).await?))
}
#[derive(Deserialize)]
struct PageQuery {
    before: Option<String>,
    limit: Option<usize>,
}
async fn items(
    State(state): State<AppState>,
    Extension(session): Extension<AuthenticatedSession>,
    Query(q): Query<PageQuery>,
) -> ApiResult<Json<relay_core::ItemPage>> {
    let a = session.actor()?.to_owned();
    Ok(Json(
        work(&state, move |r| {
            r.list_items(&a, q.before.as_deref(), q.limit.unwrap_or(100))
        })
        .await?,
    ))
}
async fn text(
    State(state): State<AppState>,
    Extension(session): Extension<AuthenticatedSession>,
    Json(input): Json<NewText>,
) -> ApiResult<Json<relay_core::Item>> {
    let a = session.actor()?.to_owned();
    Ok(Json(work(&state, move |r| r.create_text(&a, input)).await?))
}
async fn feedback(
    State(state): State<AppState>,
    Extension(session): Extension<AuthenticatedSession>,
    Json(input): Json<NewFeedback>,
) -> ApiResult<Json<relay_core::Item>> {
    let a = session.actor()?.to_owned();
    Ok(Json(
        work(&state, move |r| r.create_feedback(&a, input)).await?,
    ))
}
async fn edit_item(
    State(state): State<AppState>,
    Extension(session): Extension<AuthenticatedSession>,
    Path(id): Path<String>,
    Json(input): Json<PatchItem>,
) -> ApiResult<Json<relay_core::Item>> {
    let a = session.actor()?.to_owned();
    Ok(Json(
        work(&state, move |r| r.patch_item(&a, &id, input)).await?,
    ))
}
async fn create_upload(
    State(state): State<AppState>,
    Extension(session): Extension<AuthenticatedSession>,
    Json(input): Json<NewUpload>,
) -> ApiResult<Json<relay_core::Upload>> {
    let a = session.actor()?.to_owned();
    Ok(Json(
        work(&state, move |r| r.create_upload(&a, input)).await?,
    ))
}
async fn get_upload(
    State(state): State<AppState>,
    Extension(session): Extension<AuthenticatedSession>,
    Path(id): Path<String>,
) -> ApiResult<Json<relay_core::Upload>> {
    let a = session.actor()?.to_owned();
    Ok(Json(work(&state, move |r| r.get_upload(&a, &id)).await?))
}
async fn append_upload(
    State(state): State<AppState>,
    headers: HeaderMap,
    Extension(session): Extension<AuthenticatedSession>,
    Path(id): Path<String>,
    bytes: Bytes,
) -> ApiResult<Json<relay_core::Upload>> {
    let a = session.actor()?.to_owned();
    if headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(';').next())
        .map(str::trim)
        != Some("application/octet-stream")
    {
        return Err(ApiError(RelayError::new(
            415,
            "BINARY_REQUIRED",
            "分片必须为 application/octet-stream",
        )));
    }
    let offset = headers
        .get("upload-offset")
        .and_then(|v| v.to_str().ok())
        .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|s| s.parse::<u64>().ok())
        .ok_or_else(|| {
            ApiError(RelayError::new(
                400,
                "INVALID_OFFSET",
                "需要有效的 Upload-Offset",
            ))
        })?;
    Ok(Json(
        work(&state, move |r| r.append_upload(&a, &id, offset, &bytes)).await?,
    ))
}
async fn complete_upload(
    State(state): State<AppState>,
    Extension(session): Extension<AuthenticatedSession>,
    Path(id): Path<String>,
) -> ApiResult<Json<relay_core::Item>> {
    let a = session.actor()?.to_owned();
    Ok(Json(
        work(&state, move |r| r.complete_upload(&a, &id)).await?,
    ))
}
async fn cancel_upload(
    State(state): State<AppState>,
    Extension(session): Extension<AuthenticatedSession>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let a = session.actor()?.to_owned();
    work(&state, move |r| r.cancel_upload(&a, &id)).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn export_feedback(
    State(state): State<AppState>,
    Extension(session): Extension<AuthenticatedSession>,
    Path(id): Path<String>,
    method: Method,
) -> ApiResult<Response> {
    let a = session.actor()?.to_owned();
    let filename = format!("feedback-{id}.md");
    let text = work(&state, move |r| r.export_feedback(&a, &id)).await?;
    let length = text.len();
    let mut response = if method == Method::HEAD {
        Body::empty().into_response()
    } else {
        Body::from(text).into_response()
    };
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/markdown; charset=utf-8"),
    );
    response.headers_mut().insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&length.to_string()).unwrap(),
    );
    response
        .headers_mut()
        .insert(header::CONTENT_DISPOSITION, disposition(&filename));
    Ok(response)
}

async fn download(
    State(state): State<AppState>,
    headers: HeaderMap,
    Extension(session): Extension<AuthenticatedSession>,
    Path(id): Path<String>,
    method: Method,
) -> ApiResult<Response> {
    let a = session.actor()?.to_owned();
    let mut authorization_generation = state.relay.authorization_generation();
    let info = work(&state, move |r| r.file_info(&a, &id)).await?;
    let etag = format!("\"sha256-{}\"", info.item.sha256);
    let range = headers.get(header::RANGE).and_then(|v| v.to_str().ok());
    let if_range = headers.get(header::IF_RANGE).and_then(|v| v.to_str().ok());
    let selected = if if_range.is_none() || if_range == Some(etag.as_str()) {
        range
    } else {
        None
    };
    let (start, length, partial) = match parse_range(selected, info.item.size) {
        Ok(r) => r,
        Err(()) => {
            let mut response = ApiError(RelayError::new(
                416,
                "RANGE_NOT_SATISFIABLE",
                "下载范围不正确",
            ))
            .into_response();
            response.headers_mut().insert(
                header::CONTENT_RANGE,
                HeaderValue::from_str(&format!("bytes */{}", info.item.size)).unwrap(),
            );
            return Ok(response);
        }
    };
    let body = if method == Method::HEAD {
        Body::empty()
    } else {
        let permit = state.downloads.clone().try_acquire_owned().map_err(|_| {
            ApiError(RelayError::new(
                503,
                "DOWNLOAD_BUSY",
                "同时下载任务过多，请稍后重试",
            ))
        })?;
        let mut file = tokio::fs::File::open(info.path)
            .await
            .map_err(|e| ApiError(e.into()))?;
        file.seek(std::io::SeekFrom::Start(start))
            .await
            .map_err(|e| ApiError(e.into()))?;
        let token = bearer(&headers)?;
        let download_state = state.clone();
        Body::from_stream(async_stream::stream! {
            let _permit=permit;
            let mut remaining=length;
            let mut buffer=vec![0u8;256*1024];
            let mut checked=Instant::now();
            while remaining>0 {
                let generation=download_state.relay.authorization_generation();
                if generation != authorization_generation || checked.elapsed()>Duration::from_secs(1){
                    let credential=token.clone();
                    if work(&download_state,move|r|r.authenticate(&credential)).await.is_err(){yield Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied,"device revoked"));break;}
                    authorization_generation=generation;
                    checked=Instant::now();
                }
                let take=(remaining as usize).min(buffer.len());
                let count=match file.read(&mut buffer[..take]).await{Ok(n)=>n,Err(e)=>{yield Err(e);break;}};
                if count==0{yield Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof,"stored file truncated"));break;}
                remaining-=count as u64;
                yield Ok::<Bytes,std::io::Error>(Bytes::copy_from_slice(&buffer[..count]));
            }
        })
    };
    let mut response = body.into_response();
    *response.status_mut() = if partial {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };
    let h = response.headers_mut();
    // Arbitrary files are always attachments, not active inline web content.
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    h.insert(header::CONTENT_DISPOSITION, disposition(&info.item.name));
    h.insert(
        header::CONTENT_LENGTH,
        HeaderValue::from_str(&length.to_string()).unwrap(),
    );
    h.insert(header::ETAG, HeaderValue::from_str(&etag).unwrap());
    h.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    if partial {
        h.insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&format!(
                "bytes {}-{}/{}",
                start,
                start + length - 1,
                info.item.size
            ))
            .unwrap(),
        );
    }
    Ok(response)
}

fn disposition(name: &str) -> HeaderValue {
    HeaderValue::from_str(&format!(
        "attachment; filename=\"download\"; filename*=UTF-8''{}",
        utf8_percent_encode(name, NON_ALPHANUMERIC)
    ))
    .unwrap_or(HeaderValue::from_static(
        "attachment; filename=\"download\"",
    ))
}
fn parse_range(range: Option<&str>, size: u64) -> Result<(u64, u64, bool), ()> {
    let Some(range) = range else {
        return Ok((0, size, false));
    };
    let range = range.strip_prefix("bytes=").ok_or(())?;
    if range.contains(',') || size == 0 {
        return Err(());
    }
    let (first, last) = range.split_once('-').ok_or(())?;
    if first.is_empty() {
        let suffix = last.parse::<u64>().map_err(|_| ())?;
        if suffix == 0 {
            return Err(());
        }
        let length = suffix.min(size);
        return Ok((size - length, length, true));
    }
    let start = first.parse::<u64>().map_err(|_| ())?;
    if start >= size {
        return Err(());
    }
    let end = if last.is_empty() {
        size - 1
    } else {
        last.parse::<u64>().map_err(|_| ())?.min(size - 1)
    };
    if end < start {
        return Err(());
    }
    Ok((start, end - start + 1, true))
}

async fn events(
    State(state): State<AppState>,
    headers: HeaderMap,
    Extension(session): Extension<AuthenticatedSession>,
) -> ApiResult<impl IntoResponse> {
    let token = bearer(&headers)?;
    session.actor()?;
    // Unlike the other handlers, SSE has no core data operation to perform the
    // second revocation check before starting its long-lived response.
    let credential = token.clone();
    work(&state, move |r| r.authenticate(&credential)).await?;
    let mut receiver = state.relay.subscribe();
    let stream = async_stream::stream! {
        yield Ok::<Event,Infallible>(Event::default().event("change").data("{}"));
        let mut timer=tokio::time::interval(Duration::from_secs(15));
        loop{
            tokio::select!{_ = timer.tick()=>{},event=receiver.recv()=>{if matches!(event,Err(tokio::sync::broadcast::error::RecvError::Closed)){break;}}}
            let candidate=token.clone();
            if work(&state,move|r|r.authenticate(&candidate)).await.is_err(){break;}
            yield Ok(Event::default().event("change").data("{}"));
        }
    };
    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

#[cfg(test)]
mod tests {
    use super::parse_range;
    #[test]
    fn range_edges() {
        assert_eq!(parse_range(None, 0), Ok((0, 0, false)));
        assert_eq!(parse_range(Some("bytes=-3"), 10), Ok((7, 3, true)));
        assert_eq!(parse_range(Some("bytes=2-100"), 10), Ok((2, 8, true)));
        for s in [
            "bytes=10-",
            "bytes=3-2",
            "bytes=-0",
            "bytes=0-1,3-4",
            "none",
        ] {
            assert!(parse_range(Some(s), 10).is_err());
        }
        assert!(parse_range(Some("bytes=0-"), 0).is_err());
    }
}
