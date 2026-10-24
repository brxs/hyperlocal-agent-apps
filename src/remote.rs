//! Explicit peer access to an existing live plugin instance; no download or re-instantiation.
use anyhow::{Context, Result, bail, ensure};
use axum::{
    Router,
    body::to_bytes,
    extract::{Path, Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use crate::{
    peers::{PeerClient, peer_url, valid_digest},
    runtime::{FunctionInfo, Runtime},
    web::{self, WebMount, WebRequest, WebResponse},
};

const MAX_CALL_BYTES: usize = 1024 * 1024;
const MAX_REPLY_BYTES: u64 = 8 * 1024 * 1024;
// JSON byte arrays expand by at most four bytes per byte, plus envelope/headers.
const MAX_WEB_INPUT: usize = 5 * 1024 * 1024;
const MAX_WEB_REPLY: u64 = 40 * 1024 * 1024;

#[derive(Clone)]
struct Hosted {
    runtime: Runtime,
    name: String,
}

pub(crate) fn routes(runtime: Runtime, name: String) -> Router {
    Router::new()
        .route("/v1/plugins/{id}", get(inspect))
        .route("/v1/plugins/{id}/call", post(call))
        .route("/v1/plugins/{id}/web", post(web_request))
        .with_state(Hosted { runtime, name })
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PeerInspection {
    pub peer_name: String,
    pub plugin_id: String,
    pub sha256: String,
    pub wit: String,
    pub functions: Vec<FunctionInfo>,
    pub web_available: bool,
}

async fn inspect(State(host): State<Hosted>, Path(id): Path<String>, request: Request) -> Response {
    if let Err(error) = machine_request(&request, false) {
        return error.into_response();
    }
    match host.runtime.inspect(&id) {
        Ok(info) => json_response(
            StatusCode::OK,
            &PeerInspection {
                peer_name: host.name,
                plugin_id: id,
                sha256: info.sha256,
                wit: info.wit,
                functions: info.functions,
                web_available: info.web_url.is_some(),
            },
        ),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

#[derive(Serialize, Deserialize)]
pub struct RemoteCall {
    pub sha256: String,
    pub function: Vec<String>,
    pub arguments: serde_json::Map<String, Value>,
}
#[derive(Serialize, Deserialize)]
struct RemoteWeb {
    sha256: String,
    request: WebRequest,
}

// This is a machine API. Browser access goes through a loopback origin-checked proxy.
fn machine_request(request: &Request, json_body: bool) -> std::result::Result<(), StatusCode> {
    if request.headers().contains_key(header::ORIGIN)
        || request
            .headers()
            .keys()
            .any(|h| h.as_str().starts_with("sec-fetch-"))
    {
        return Err(StatusCode::FORBIDDEN);
    }
    if json_body
        && request
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.split(';').next())
            .map(str::trim)
            != Some("application/json")
    {
        return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    Ok(())
}

async fn input<T: DeserializeOwned>(
    request: Request,
    limit: usize,
) -> std::result::Result<T, Response> {
    machine_request(&request, true).map_err(IntoResponse::into_response)?;
    let body = tokio::time::timeout(
        Duration::from_secs(10),
        to_bytes(request.into_body(), limit),
    )
    .await
    .map_err(|_| StatusCode::REQUEST_TIMEOUT.into_response())?
    .map_err(|_| StatusCode::PAYLOAD_TOO_LARGE.into_response())?;
    serde_json::from_slice(&body)
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid JSON request").into_response())
}

async fn call(State(host): State<Hosted>, Path(id): Path<String>, request: Request) -> Response {
    let input: RemoteCall = match input(request, MAX_CALL_BYTES).await {
        Ok(value) => value,
        Err(error) => return error,
    };
    let name = host.name;
    let reply_id = id.clone();
    let digest = input.sha256.clone();
    let result = tokio::task::spawn_blocking(move || {
        host.runtime
            .call_versioned(&id, &input.sha256, &input.function, &input.arguments)
    })
    .await;
    match result {
        Ok(Ok(result)) => json_response(
            StatusCode::OK,
            &json!({"peer_name": name, "plugin_id": reply_id, "sha256": digest, "result": result}),
        ),
        Ok(Err(error)) => failure(error),
        Err(error) => failure(error.into()),
    }
}

async fn web_request(
    State(host): State<Hosted>,
    Path(id): Path<String>,
    request: Request,
) -> Response {
    let input: RemoteWeb = match input(request, MAX_WEB_INPUT).await {
        Ok(value) => value,
        Err(error) => return error,
    };
    if input.request.body.len() > web::MAX_REQUEST_BYTES {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    }
    let result = tokio::task::spawn_blocking(move || {
        let arguments = json!({"request": input.request})
            .as_object()
            .unwrap()
            .clone();
        web::decode_response(host.runtime.call_versioned(
            &id,
            &input.sha256,
            &[web::INTERFACE.into(), web::FUNCTION.into()],
            &arguments,
        )?)
    })
    .await;
    match result {
        Ok(Ok(response)) => json_response(StatusCode::OK, &response),
        Ok(Err(error)) => failure(error),
        Err(error) => failure(error.into()),
    }
}

fn failure(error: anyhow::Error) -> Response {
    json_response(
        StatusCode::UNPROCESSABLE_ENTITY,
        &json!({"error": format!("{error:#}")}),
    )
}
fn json_response(status: StatusCode, value: &impl Serialize) -> Response {
    match serde_json::to_vec(value) {
        Ok(bytes) => (
            status,
            [
                (header::CONTENT_TYPE, "application/json"),
                (header::CACHE_CONTROL, "no-store"),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

fn endpoint(base: &str, id: &str, operation: Option<&str>) -> Result<reqwest::Url> {
    ensure!(
        !id.is_empty()
            && id.len() <= 128
            && id != "."
            && id != ".."
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)),
        "invalid peer plugin id"
    );
    let mut url = peer_url(base)?;
    {
        let mut path = url.path_segments_mut().unwrap();
        path.clear().push("v1").push("plugins").push(id);
        if let Some(operation) = operation {
            path.push(operation);
        }
    }
    Ok(url)
}

impl PeerClient {
    pub async fn inspect_remote(&self, base: &str, id: &str) -> Result<PeerInspection> {
        let bytes = self
            .fetch(endpoint(base, id, None)?, MAX_REPLY_BYTES)
            .await
            .context("peer interface inspection failed; the publisher must enable --share-live")?;
        let info: PeerInspection =
            serde_json::from_slice(&bytes).context("invalid peer interface description")?;
        ensure!(
            info.peer_name.len() <= 128 && info.plugin_id == id && valid_digest(&info.sha256),
            "invalid peer identity or component digest"
        );
        Ok(info)
    }

    pub async fn call_remote(&self, base: &str, id: &str, call: &RemoteCall) -> Result<Value> {
        ensure!(
            valid_digest(&call.sha256),
            "invalid SHA-256; inspect_peer_plugin first"
        );
        let bytes = serde_json::to_vec(call)?;
        ensure!(bytes.len() <= MAX_CALL_BYTES, "remote call exceeds 1 MiB");
        self.post(endpoint(base, id, Some("call"))?, bytes, MAX_REPLY_BYTES)
            .await
    }

    async fn post(&self, url: reqwest::Url, bytes: Vec<u8>, limit: u64) -> Result<Value> {
        let mut response = self
            .client
            .post(url)
            .header(header::CONTENT_TYPE, "application/json")
            .body(bytes)
            .send()
            .await
            .context(
                "peer request failed; mutation outcome may be unknown, do not automatically retry",
            )?;
        let status = response.status();
        if let Some(length) = response.content_length() {
            ensure!(length <= limit, "peer response exceeds size limit");
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .context("peer response interrupted; mutation outcome may be unknown")?
        {
            ensure!(
                (bytes.len() + chunk.len()) as u64 <= limit,
                "peer response exceeds size limit"
            );
            bytes.extend_from_slice(&chunk);
        }
        let value: Value = serde_json::from_slice(&bytes).with_context(|| {
            format!("peer returned HTTP {status} without a valid JSON response")
        })?;
        if !status.is_success() {
            bail!(
                "peer returned HTTP {status}: {}",
                value
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("request failed")
            );
        }
        Ok(value)
    }

    async fn forward_web(
        &self,
        base: &str,
        id: &str,
        digest: &str,
        mut request: WebRequest,
    ) -> Result<WebResponse> {
        // Never forward ambient local cookies, auth, origin or transport headers to a peer.
        request.headers.retain(|(name, _)| {
            let name = name.to_ascii_lowercase();
            !matches!(
                name.as_str(),
                "cookie"
                    | "authorization"
                    | "proxy-authorization"
                    | "host"
                    | "origin"
                    | "referer"
                    | "connection"
                    | "content-length"
                    | "transfer-encoding"
                    | "forwarded"
            ) && !name.starts_with("sec-fetch-")
                && !name.starts_with("x-forwarded-")
        });
        let value = self
            .post(
                endpoint(base, id, Some("web"))?,
                serde_json::to_vec(&RemoteWeb {
                    sha256: digest.to_owned(),
                    request,
                })?,
                MAX_WEB_REPLY,
            )
            .await?;
        let mut response = web::decode_response(json!({"results": [value]}))?;
        response
            .headers
            .retain(|(name, _)| !name.eq_ignore_ascii_case("set-cookie"));
        Ok(response)
    }
}

#[derive(Clone, Serialize)]
pub struct RemoteWebConnection {
    pub peer_name: String,
    pub peer_url: String,
    pub plugin_id: String,
    pub sha256: String,
    pub web_url: String,
}
struct View {
    info: RemoteWebConnection,
    _mount: WebMount,
}
#[derive(Clone, Default)]
pub struct RemoteViews {
    views: Arc<Mutex<BTreeMap<(String, String), View>>>,
}
impl RemoteViews {
    pub async fn open(
        &self,
        client: PeerClient,
        base: &str,
        id: &str,
    ) -> Result<RemoteWebConnection> {
        let info = client.inspect_remote(base, id).await?;
        ensure!(
            info.web_available,
            "{} / {id} has no web interface",
            info.peer_name
        );
        let base = peer_url(base)?.to_string();
        let key = (base.clone(), id.to_owned());
        let mut views = self
            .views
            .lock()
            .map_err(|_| anyhow::anyhow!("peer web connection lock poisoned"))?;
        if let Some(view) = views
            .get_mut(&key)
            .filter(|view| view.info.sha256 == info.sha256)
        {
            view.info.peer_name = info.peer_name;
            return Ok(view.info.clone());
        }
        let remote = (base.clone(), id.to_owned(), info.sha256.clone());
        let metadata = json!({"peer_name": info.peer_name, "peer_url": base, "plugin_id": id, "sha256": info.sha256});
        let mount = WebMount::start_async(move |request| {
            let metadata = metadata.clone();
            let client = client.clone();
            let (base, id, digest) = remote.clone();
            async move {
                if request.path == "/.hyperlocal/connection" {
                    return Ok(WebResponse {
                        status: if request.method == "GET" { 200 } else { 405 },
                        headers: vec![
                            ("content-type".into(), "application/json".into()),
                            ("cache-control".into(), "no-store".into()),
                        ],
                        body: serde_json::to_vec(&metadata)?,
                    });
                }
                client.forward_web(&base, &id, &digest, request).await
            }
        })?;
        let info = RemoteWebConnection {
            peer_name: info.peer_name,
            peer_url: base,
            plugin_id: id.to_owned(),
            sha256: info.sha256,
            web_url: mount.url.clone(),
        };
        views.insert(
            key,
            View {
                info: info.clone(),
                _mount: mount,
            },
        );
        Ok(info)
    }
    pub fn close(&self, web_url: &str) -> Result<()> {
        let mut views = self
            .views
            .lock()
            .map_err(|_| anyhow::anyhow!("peer web connection lock poisoned"))?;
        let key = views
            .iter()
            .find(|(_, v)| v.info.web_url == web_url)
            .map(|(k, _)| k.clone())
            .context("unknown peer web connection")?;
        views.remove(&key);
        Ok(())
    }
}
