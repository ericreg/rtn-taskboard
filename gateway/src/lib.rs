#![forbid(unsafe_code)]

use anyhow::{Context, anyhow, bail, ensure};
use axum::{
    Router,
    body::{Body, Bytes},
    extract::{ConnectInfo, DefaultBodyLimit, Request, State},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{any, get},
};
use http_body_util::BodyExt;
use rtn_mq::{
    Identity, JoinCode, MessagingEndpoint, Nack, PublishOptions, Publisher, RecipientOutcome,
    Subscription, SubscriptionOptions,
};
use std::{
    collections::HashMap,
    io,
    net::SocketAddr,
    path::{Path, PathBuf},
    str::FromStr,
    sync::Arc,
    time::Duration,
};
use taskboard_wire::{
    BODY_CHUNK_BYTES, Envelope, FRAME_FORMAT, Frame, Header, REQUEST_TOPIC, RESPONSE_TOPIC,
    RequestId,
};
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio_stream::wrappers::ReceiverStream;
use tower_http::{
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(120);
const MAX_IN_FLIGHT: usize = 128;

#[derive(Clone)]
pub struct Config {
    pub bind: String,
    pub frontend: PathBuf,
    pub identity: PathBuf,
    pub join_code: JoinCode,
    pub relay_only: bool,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let get =
            |key: &str, default: &str| std::env::var(key).unwrap_or_else(|_| default.to_owned());
        let encoded = match std::env::var("TASKBOARD_RTN_JOIN_CODE") {
            Ok(value) if !value.trim().is_empty() => value,
            _ => {
                let path = std::env::var("TASKBOARD_RTN_JOIN_CODE_FILE").context(
                    "TASKBOARD_RTN_JOIN_CODE or TASKBOARD_RTN_JOIN_CODE_FILE is required",
                )?;
                std::fs::read_to_string(path).context("read Taskboard join-code file")?
            }
        };
        Ok(Self {
            bind: get("TASKBOARD_GATEWAY_BIND", "0.0.0.0:8080"),
            frontend: get("TASKBOARD_FRONTEND", "frontend/dist").into(),
            identity: get("TASKBOARD_RTN_IDENTITY", "data/rtn/gateway.key").into(),
            join_code: JoinCode::decode(encoded.trim()).context("decode Taskboard join code")?,
            relay_only: get("TASKBOARD_RTN_RELAY_ONLY", "true")
                .parse()
                .context("Invalid TASKBOARD_RTN_RELAY_ONLY")?,
        })
    }
}

#[derive(Clone)]
pub struct GatewayState {
    endpoint: MessagingEndpoint,
    requests: Publisher,
    host: rtn_mq::EndpointId,
    pending: Arc<Mutex<HashMap<RequestId, PendingResponse>>>,
}

impl GatewayState {
    pub fn endpoint(&self) -> &MessagingEndpoint {
        &self.endpoint
    }
}

struct PendingResponse {
    head: Option<oneshot::Sender<Result<ResponseHead, String>>>,
    body: Option<mpsc::Sender<Result<Bytes, io::Error>>>,
    next_sequence: u64,
}

struct ResponseHead {
    status: StatusCode,
    headers: HeaderMap,
    body: mpsc::Receiver<Result<Bytes, io::Error>>,
}

pub async fn connect(config: &Config) -> anyhow::Result<(GatewayState, Subscription, JoinCode)> {
    let identity = load_or_create_identity(&config.identity)?;
    let endpoint = loop {
        match MessagingEndpoint::join(
            transport_config(config.relay_only),
            identity.clone(),
            &config.join_code,
        )
        .await
        {
            Ok(endpoint) => break endpoint,
            Err(error) => {
                tracing::warn!(%error, "Taskboard backend unavailable; retrying join");
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    };
    endpoint.online(Duration::from_secs(30)).await?;
    let host = config.join_code.host_id();
    let mut responses = endpoint
        .subscribe(RESPONSE_TOPIC, SubscriptionOptions::acknowledged())
        .await?;
    responses
        .wait_ready(host, Duration::from_secs(30))
        .await
        .context("backend did not accept the response subscription")?;
    let requests = endpoint.publisher(REQUEST_TOPIC)?;
    Ok((
        GatewayState {
            endpoint,
            requests,
            host,
            pending: Arc::new(Mutex::new(HashMap::new())),
        },
        responses,
        config.join_code.clone(),
    ))
}

pub fn router(state: GatewayState, frontend: PathBuf) -> Router {
    let static_files =
        ServeDir::new(&frontend).not_found_service(ServeFile::new(frontend.join("index.html")));
    Router::new()
        .route("/health/live", get(|| async { "ok" }))
        .route("/health/ready", get(ready))
        .route("/api/v1", any(proxy))
        .route("/api/v1/{*path}", any(proxy))
        .fallback_service(static_files)
        .layer(DefaultBodyLimit::disable())
        .layer(middleware::from_fn(security_headers))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

pub async fn response_loop(mut responses: Subscription, state: GatewayState) -> anyhow::Result<()> {
    while let Some(delivery) = responses.recv().await? {
        if delivery.format != FRAME_FORMAT {
            delivery.nack(Nack::Permanent).await?;
            continue;
        }
        let envelope = match Envelope::decode(delivery.payload()) {
            Ok(envelope) => envelope,
            Err(error) => {
                tracing::warn!(%error, "rejected malformed tunnel response");
                delivery.nack(Nack::Permanent).await?;
                continue;
            }
        };
        accept_response(envelope, &state.pending).await;
        delivery.ack().await?;
    }
    fail_pending(&state.pending, "Taskboard backend connection closed").await;
    bail!("Taskboard tunnel response subscription closed")
}

pub async fn reconnect_loop(endpoint: MessagingEndpoint, code: JoinCode) {
    let mut interval = tokio::time::interval(Duration::from_secs(5));
    loop {
        interval.tick().await;
        let disconnected = endpoint
            .metrics()
            .await
            .map(|metrics| metrics.peers == 0)
            .unwrap_or(true);
        if disconnected {
            tracing::warn!("Taskboard backend disconnected; attempting to rejoin");
            if let Err(error) = endpoint.rejoin(&code).await {
                tracing::warn!(%error, "Taskboard backend rejoin failed");
            }
        }
    }
}

async fn proxy(
    State(state): State<GatewayState>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
) -> Response {
    match forward(state, peer, request).await {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(%error, "gateway request failed");
            unavailable()
        }
    }
}

async fn forward(
    state: GatewayState,
    peer: SocketAddr,
    request: Request,
) -> anyhow::Result<Response> {
    let (parts, mut body) = request.into_parts();
    let method = parts.method.as_str().to_owned();
    let path_and_query = parts
        .uri
        .path_and_query()
        .map(|value| value.as_str())
        .unwrap_or(parts.uri.path())
        .to_owned();
    let headers = encode_headers(&parts.headers)?;
    let (head_tx, head_rx) = oneshot::channel();
    let request_id = {
        let mut pending = state.pending.lock().await;
        ensure!(
            pending.len() < MAX_IN_FLIGHT,
            "gateway concurrency limit reached"
        );
        loop {
            let request_id = rand::random();
            if let std::collections::hash_map::Entry::Vacant(entry) = pending.entry(request_id) {
                entry.insert(PendingResponse {
                    head: Some(head_tx),
                    body: None,
                    next_sequence: 0,
                });
                break request_id;
            }
        }
    };

    let result = async {
        send_frame(
            &state,
            Envelope::new(Frame::RequestStart {
                request_id,
                method,
                path_and_query,
                headers,
                client_ip: peer.ip().to_string(),
            }),
        )
        .await?;
        let mut sequence = 0;
        while let Some(frame) = tokio::time::timeout(RESPONSE_TIMEOUT, body.frame())
            .await
            .context("browser request body timed out")?
        {
            let frame = frame.context("read browser request body")?;
            if let Ok(data) = frame.into_data() {
                for chunk in data.chunks(BODY_CHUNK_BYTES) {
                    if chunk.is_empty() {
                        continue;
                    }
                    send_frame(
                        &state,
                        Envelope::new(Frame::RequestChunk {
                            request_id,
                            sequence,
                            bytes: chunk.to_vec(),
                        }),
                    )
                    .await?;
                    sequence += 1;
                }
            }
        }
        send_frame(
            &state,
            Envelope::new(Frame::RequestEnd {
                request_id,
                chunks: sequence,
            }),
        )
        .await?;
        let head = tokio::time::timeout(RESPONSE_TIMEOUT, head_rx)
            .await
            .context("backend response timed out")?
            .context("backend response channel closed")?
            .map_err(|error| anyhow!(error))?;
        let mut response = Response::new(Body::from_stream(ReceiverStream::new(head.body)));
        *response.status_mut() = head.status;
        *response.headers_mut() = head.headers;
        Ok(response)
    }
    .await;

    if result.is_err() {
        state.pending.lock().await.remove(&request_id);
        let _ = send_frame(&state, Envelope::new(Frame::RequestCancel { request_id })).await;
    }
    result
}

async fn send_frame(state: &GatewayState, envelope: Envelope) -> anyhow::Result<()> {
    let bytes = envelope.encode().context("encode tunnel frame")?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    loop {
        let payload = state.endpoint.buffers().copy_from_slice(&bytes)?;
        let mut receipt = state
            .requests
            .publish(
                payload,
                PublishOptions {
                    lifetime: Duration::from_secs(120),
                    format: FRAME_FORMAT.into(),
                    ..Default::default()
                },
            )
            .await?;
        let outcomes = receipt
            .wait_for_processing(Duration::from_secs(120))
            .await?;
        if outcomes.len() == 1
            && outcomes[0].0 == state.host
            && matches!(outcomes[0].1, RecipientOutcome::Processed)
        {
            return Ok(());
        }
        if outcomes.is_empty() && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        }
        bail!("backend did not acknowledge tunnel frame: {outcomes:?}");
    }
}

async fn accept_response(envelope: Envelope, pending: &Mutex<HashMap<RequestId, PendingResponse>>) {
    match envelope.frame {
        Frame::ResponseStart {
            request_id,
            status,
            headers,
        } => {
            let parsed = parse_response_head(status, headers);
            let mut requests = pending.lock().await;
            let Some(request) = requests.get_mut(&request_id) else {
                return;
            };
            let Some(sender) = request.head.take() else {
                return;
            };
            match parsed {
                Ok((status, headers)) => {
                    let (body_tx, body_rx) = mpsc::channel(4);
                    request.body = Some(body_tx);
                    if sender
                        .send(Ok(ResponseHead {
                            status,
                            headers,
                            body: body_rx,
                        }))
                        .is_err()
                    {
                        requests.remove(&request_id);
                    }
                }
                Err(error) => {
                    let _ = sender.send(Err(error.to_string()));
                    requests.remove(&request_id);
                }
            }
        }
        Frame::ResponseChunk {
            request_id,
            sequence,
            bytes,
        } => {
            let sender = {
                let mut requests = pending.lock().await;
                let Some(request) = requests.get_mut(&request_id) else {
                    return;
                };
                if sequence != request.next_sequence {
                    requests.remove(&request_id);
                    return;
                }
                request.next_sequence += 1;
                request.body.clone()
            };
            if let Some(sender) = sender
                && sender.send(Ok(bytes.into())).await.is_err()
            {
                pending.lock().await.remove(&request_id);
            }
        }
        Frame::ResponseEnd { request_id, chunks } => {
            let request = pending.lock().await.remove(&request_id);
            if let Some(mut request) = request
                && request.next_sequence != chunks
            {
                let message = "backend response ended with a missing frame";
                if let Some(head) = request.head.take() {
                    let _ = head.send(Err(message.into()));
                }
                if let Some(body) = request.body.take() {
                    let _ =
                        body.try_send(Err(io::Error::new(io::ErrorKind::UnexpectedEof, message)));
                }
            }
        }
        Frame::RequestStart { .. }
        | Frame::RequestChunk { .. }
        | Frame::RequestEnd { .. }
        | Frame::RequestCancel { .. } => {}
    }
}

fn parse_response_head(
    status: u16,
    headers: Vec<Header>,
) -> anyhow::Result<(StatusCode, HeaderMap)> {
    let status = StatusCode::from_u16(status)?;
    let mut parsed = HeaderMap::new();
    for value in headers {
        let name = HeaderName::from_str(&value.name)?;
        if is_hop_by_hop(&name) {
            continue;
        }
        parsed.append(name, HeaderValue::from_bytes(&value.value)?);
    }
    Ok((status, parsed))
}

fn encode_headers(headers: &HeaderMap) -> anyhow::Result<Vec<Header>> {
    headers
        .iter()
        .filter(|(name, _)| !is_hop_by_hop(name))
        .map(|(name, value)| {
            Ok(Header {
                name: name.as_str().to_owned(),
                value: value.as_bytes().to_vec(),
            })
        })
        .collect()
}

fn is_hop_by_hop(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

async fn ready(State(state): State<GatewayState>) -> Response {
    match state.endpoint.metrics().await {
        Ok(metrics) if metrics.peers == 1 => "ok".into_response(),
        _ => unavailable(),
    }
}

async fn security_headers(request: Request, next: Next) -> Response {
    let api = request.uri().path().starts_with("/api/");
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("referrer-policy", HeaderValue::from_static("same-origin"));
    headers.insert("x-frame-options", HeaderValue::from_static("DENY"));
    headers.insert(
        "content-security-policy",
        HeaderValue::from_static(
            "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' blob:; connect-src 'self'; frame-ancestors 'none'; base-uri 'self'; form-action 'self'; object-src 'none'",
        ),
    );
    if api {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}

fn unavailable() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        [(header::RETRY_AFTER, "5")],
        "Taskboard backend unavailable",
    )
        .into_response()
}

async fn fail_pending(pending: &Mutex<HashMap<RequestId, PendingResponse>>, message: &str) {
    let requests = std::mem::take(&mut *pending.lock().await);
    for (_, mut request) in requests {
        if let Some(head) = request.head.take() {
            let _ = head.send(Err(message.to_owned()));
        }
        if let Some(body) = request.body.take() {
            let _ = body.try_send(Err(io::Error::new(io::ErrorKind::ConnectionReset, message)));
        }
    }
}

fn transport_config(relay_only: bool) -> rtn_mq::Config {
    let mut config = rtn_mq::Config::new();
    config.relay_only = relay_only;
    config.max_peers = 1;
    config.max_topics = 2;
    config
}

fn load_or_create_identity(path: &Path) -> anyhow::Result<Identity> {
    if path.exists() {
        return Identity::load(path).context("load gateway rtn-mq identity");
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let existed = parent.exists();
    std::fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if !existed {
            std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
        }
        let metadata = std::fs::symlink_metadata(parent)?;
        ensure!(
            metadata.is_dir()
                && !metadata.file_type().is_symlink()
                && metadata.permissions().mode() & 0o077 == 0,
            "gateway identity directory must be a private (0700) real directory"
        );
    }
    let identity = Identity::generate();
    identity.save(path)?;
    Ok(identity)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{extract::ConnectInfo, http::Request};
    use http_body_util::BodyExt;
    use rtn_mq::{JoinOptions, Permission, RelayMode, ShutdownMode, SubscriptionOptions};
    use serde_json::{Value, json};
    use taskboard::{auth, config::Config as BackendConfig, state::AppState};
    use tower::ServiceExt;

    #[test]
    fn strips_hop_by_hop_headers() {
        assert!(is_hop_by_hop(&header::CONNECTION));
        assert!(is_hop_by_hop(&header::TRANSFER_ENCODING));
        assert!(!is_hop_by_hop(&header::COOKIE));
        assert!(!is_hop_by_hop(&header::SET_COOKIE));
    }

    fn local_transport() -> rtn_mq::Config {
        let mut config = rtn_mq::Config::new();
        config.relay_mode = RelayMode::Disabled;
        config.bind_addr = Some("127.0.0.1:0".parse().unwrap());
        config.relay_only = false;
        config.max_peers = 1;
        config.max_topics = 2;
        config.retry_interval = Duration::from_millis(50);
        config
    }

    async fn call(app: &Router, request: Request<Body>) -> Response {
        let mut request = request;
        request.extensions_mut().insert(ConnectInfo(
            "127.0.0.1:40000".parse::<SocketAddr>().unwrap(),
        ));
        app.clone().oneshot(request).await.unwrap()
    }

    #[tokio::test]
    async fn crud_and_large_images_cross_the_signed_tunnel() {
        let directory = tempfile::tempdir().unwrap();
        let backend_state = AppState::new(BackendConfig {
            database: directory.path().join("taskboard.db"),
            base_url: "http://taskboard.test".into(),
            max_db_bytes: 0,
            max_image_bytes: 10 * 1024 * 1024,
            secure_cookies: false,
            discord_token: None,
            discord_guild: None,
            rtn_identity: directory.path().join("backend.key"),
            rtn_state: directory.path().join("backend.cbor"),
            rtn_relay_only: false,
        })
        .await
        .unwrap();
        auth::bootstrap(
            &backend_state,
            "admin@example.test",
            "Admin",
            "a long test-only passphrase",
        )
        .await
        .unwrap();
        let (token, csrf) = auth::new_session(&backend_state, 1).await.unwrap();

        let backend = MessagingEndpoint::host(
            local_transport(),
            Identity::generate(),
            vec![
                Permission::subscribe(REQUEST_TOPIC).unwrap(),
                Permission::publish(RESPONSE_TOPIC).unwrap(),
            ],
        )
        .await
        .unwrap();
        let mut options = JoinOptions::new(vec![
            Permission::publish(REQUEST_TOPIC).unwrap(),
            Permission::subscribe(RESPONSE_TOPIC).unwrap(),
        ]);
        options.max_uses = 1;
        let code = backend.issue_join_code(options).await.unwrap();
        let backend_task = tokio::spawn(taskboard::tunnel::run(
            backend.clone(),
            backend_state.clone(),
        ));
        tokio::task::yield_now().await;

        let endpoint = MessagingEndpoint::join(local_transport(), Identity::generate(), &code)
            .await
            .unwrap();
        let mut responses = endpoint
            .subscribe(RESPONSE_TOPIC, SubscriptionOptions::acknowledged())
            .await
            .unwrap();
        responses
            .wait_ready(backend.endpoint_id(), Duration::from_secs(5))
            .await
            .unwrap();
        let state = GatewayState {
            requests: endpoint.publisher(REQUEST_TOPIC).unwrap(),
            host: backend.endpoint_id(),
            endpoint,
            pending: Arc::new(Mutex::new(HashMap::new())),
        };
        let response_task = tokio::spawn(response_loop(responses, state.clone()));
        let app = router(state.clone(), directory.path().into());

        // Login loads these endpoints concurrently. Exercise that burst with
        // authenticated requests and consume every body, not just its headers.
        let mut initial_requests = tokio::task::JoinSet::new();
        for path in ["auth/me", "projects", "users", "status", "notifications", "tasks"] {
            let app = app.clone();
            let cookie = format!("taskboard_session={token}");
            initial_requests.spawn(async move {
                let response = call(
                    &app,
                    Request::builder()
                        .uri(format!("/api/v1/{path}"))
                        .header(header::COOKIE, cookie)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await;
                assert_eq!(response.status(), StatusCode::OK, "{path}");
                let bytes = response.into_body().collect().await.unwrap().to_bytes();
                serde_json::from_slice::<Value>(&bytes).unwrap();
            });
        }
        tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(result) = initial_requests.join_next().await {
                result.unwrap();
            }
        })
        .await
        .expect("concurrent post-login API requests stalled");

        let project = call(
            &app,
            Request::builder()
                .method("POST")
                .uri("/api/v1/projects")
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ORIGIN, "http://taskboard.test")
                .header(header::COOKIE, format!("taskboard_session={token}"))
                .header("x-csrf-token", &csrf)
                .body(Body::from(json!({"name":"Through the tunnel"}).to_string()))
                .unwrap(),
        )
        .await;
        assert_eq!(project.status(), StatusCode::OK);
        let project: Value =
            serde_json::from_slice(&project.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        let project_id = project["id"].as_i64().unwrap();

        // The configured maximum still exceeds the rtn-mq frame ceiling, so both
        // tunnel directions must stream it without changing the application limit.
        let image_size = 10 * 1024 * 1024;
        let mut image = vec![0; image_size];
        image[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
        let mut multipart = b"--image-boundary\r\nContent-Disposition: form-data; name=\"file\"; filename=\"large.png\"\r\nContent-Type: image/png\r\n\r\n".to_vec();
        multipart.extend_from_slice(&image);
        multipart.extend_from_slice(b"\r\n--image-boundary--\r\n");
        let upload = call(
            &app,
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/projects/{project_id}/attachments"))
                .header(
                    header::CONTENT_TYPE,
                    "multipart/form-data; boundary=image-boundary",
                )
                .header(header::ORIGIN, "http://taskboard.test")
                .header(header::COOKIE, format!("taskboard_session={token}"))
                .header("x-csrf-token", &csrf)
                .body(Body::from(multipart))
                .unwrap(),
        )
        .await;
        assert_eq!(upload.status(), StatusCode::OK);
        let upload: Value =
            serde_json::from_slice(&upload.into_body().collect().await.unwrap().to_bytes())
                .unwrap();
        assert_eq!(upload["size"], image_size);

        let download = call(
            &app,
            Request::builder()
                .uri(format!(
                    "/api/v1/attachments/{}",
                    upload["id"].as_str().unwrap()
                ))
                .header(header::COOKIE, format!("taskboard_session={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(download.status(), StatusCode::OK);
        assert_eq!(
            download.into_body().collect().await.unwrap().to_bytes(),
            image.as_slice()
        );

        response_task.abort();
        backend_task.abort();
        state
            .endpoint
            .shutdown(ShutdownMode::Immediate)
            .await
            .unwrap();
        backend.shutdown(ShutdownMode::Immediate).await.unwrap();
    }
}
