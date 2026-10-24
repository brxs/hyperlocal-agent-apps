//! Optional WIT HTTP export and loopback mount lifecycle.
use std::{future::Future, net::TcpListener, pin::Pin, sync::Arc, time::Duration};

use anyhow::{Context, Result, ensure};
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{HeaderName, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::oneshot;

use crate::runtime::FunctionInfo;

pub const INTERFACE: &str = "agent:web/handler@0.1.0";
pub const FUNCTION: &str = "handle";
pub(crate) const MAX_REQUEST_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

type Handler = Arc<
    dyn Fn(WebRequest) -> Pin<Box<dyn Future<Output = Result<WebResponse>> + Send>> + Send + Sync,
>;

#[derive(Serialize, Deserialize)]
pub struct WebRequest {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WebResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Validate by structural WIT type, rather than accepting any similarly named function.
pub fn validate(functions: &[FunctionInfo]) -> Result<bool> {
    let path = vec![INTERFACE.to_owned(), FUNCTION.to_owned()];
    let Some(function) = functions.iter().find(|f| f.path == path) else {
        return Ok(false);
    };
    let headers = json!({"list": {"tuple": ["string", "string"]}});
    let request = json!({"record": {"method": "string", "path": "string", "headers": headers, "body": {"list": "u8"}}});
    let response = json!({"record": {"status": "u16", "headers": headers, "body": {"list": "u8"}}});
    ensure!(
        function.parameters.len() == 1
            && function.parameters[0].name == "request"
            && function.parameters[0].ty == request
            && function.results == vec![response],
        "{INTERFACE}.{FUNCTION} must match wit/web.wit"
    );
    Ok(true)
}

pub fn decode_response(value: Value) -> Result<WebResponse> {
    let result = value
        .get("results")
        .and_then(Value::as_array)
        .and_then(|r| r.first())
        .context("web handler returned no response")?;
    let response: WebResponse =
        serde_json::from_value(result.clone()).context("invalid web response")?;
    ensure!(
        (200..=599).contains(&response.status),
        "web response must have a final HTTP status between 200 and 599"
    );
    ensure!(
        response.body.len() <= MAX_RESPONSE_BYTES,
        "web response exceeds 8 MiB"
    );
    Ok(response)
}

/// A plugin owns its listener. Dropping the last mount closes the listener.
/// The handler must capture the registry weakly to avoid a registry/mount cycle.
pub struct WebMount {
    pub url: String,
    stop: Option<oneshot::Sender<()>>,
}

impl WebMount {
    pub fn start(
        handler: impl Fn(WebRequest) -> Result<WebResponse> + Send + Sync + 'static,
    ) -> Result<Self> {
        let handler = Arc::new(handler);
        Self::start_async(move |request| {
            let handler = handler.clone();
            async move { tokio::task::spawn_blocking(move || handler(request)).await? }
        })
    }

    pub fn start_async<F, Fut>(handler: F) -> Result<Self>
    where
        F: Fn(WebRequest) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<WebResponse>> + Send + 'static,
    {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .context("cannot bind plugin web listener")?;
        listener.set_nonblocking(true)?;
        let authority = listener.local_addr()?.to_string();
        let url = format!("http://{authority}/");
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let (stop, stopped) = oneshot::channel();
        let state = HttpState {
            handler: Arc::new(move |request| Box::pin(handler(request))),
            origin: format!("http://{authority}"),
            authority,
        };
        std::thread::Builder::new()
            .name("plugin-web".into())
            .spawn(move || {
                executor.block_on(async move {
                    let result = async {
                        let listener = tokio::net::TcpListener::from_std(listener)?;
                        let router = Router::new().fallback(dispatch).with_state(state);
                        axum::serve(listener, router)
                            .with_graceful_shutdown(async {
                                let _ = stopped.await;
                            })
                            .await
                    }
                    .await;
                    if let Err(error) = result {
                        eprintln!("plugin web listener failed: {error}");
                    }
                });
            })?;
        Ok(Self {
            url,
            stop: Some(stop),
        })
    }
}

impl Drop for WebMount {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

#[derive(Clone)]
struct HttpState {
    handler: Handler,
    authority: String,
    origin: String,
}

async fn dispatch(State(state): State<HttpState>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    // Keep arbitrary DNS names and other browser origins from using this local listener.
    if parts
        .headers
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        != Some(state.authority.as_str())
    {
        return (StatusCode::FORBIDDEN, "Unexpected Host").into_response();
    }
    if parts
        .headers
        .get(header::ORIGIN)
        .is_some_and(|h| h.to_str().ok() != Some(state.origin.as_str()))
    {
        return (StatusCode::FORBIDDEN, "Cross-origin request denied").into_response();
    }
    if parts
        .headers
        .get("sec-fetch-site")
        .is_some_and(|h| h == "cross-site")
    {
        let navigation = parts
            .headers
            .get("sec-fetch-mode")
            .is_some_and(|h| h == "navigate");
        if !navigation || (parts.method != "GET" && parts.method != "HEAD") {
            return (StatusCode::FORBIDDEN, "Cross-site request denied").into_response();
        }
    }
    let body = match tokio::time::timeout(
        Duration::from_secs(10),
        to_bytes(body, MAX_REQUEST_BYTES),
    )
    .await
    {
        Ok(Ok(body)) => body,
        Ok(Err(_)) => {
            return (StatusCode::PAYLOAD_TOO_LARGE, "Request body exceeds 1 MiB").into_response();
        }
        Err(_) => return StatusCode::REQUEST_TIMEOUT.into_response(),
    };
    let head = parts.method == "HEAD";
    let request = WebRequest {
        method: parts.method.to_string(),
        path: parts
            .uri
            .path_and_query()
            .map(|p| p.as_str())
            .unwrap_or("/")
            .to_owned(),
        headers: parts
            .headers
            .iter()
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|v| (name.to_string(), v.to_owned()))
            })
            .collect(),
        body: body.to_vec(),
    };
    let result = (state.handler)(request).await;
    match result {
        Ok(web) => match into_response(web, head) {
            Ok(response) => response,
            Err(error) => {
                eprintln!("invalid plugin HTTP response: {error:#}");
                (StatusCode::BAD_GATEWAY, "Invalid plugin response").into_response()
            }
        },
        Err(error) => {
            eprintln!("plugin HTTP handler failed: {error:#}");
            (StatusCode::INTERNAL_SERVER_ERROR, "Plugin handler failed").into_response()
        }
    }
}

fn into_response(web: WebResponse, head: bool) -> Result<Response> {
    let mut response = Response::builder().status(web.status);
    for (name, value) in web.headers {
        let name = HeaderName::try_from(name)?;
        // The HTTP server owns transport framing. Components cannot enable cross-origin access.
        if matches!(
            name.as_str(),
            "connection"
                | "content-length"
                | "transfer-encoding"
                | "keep-alive"
                | "upgrade"
                | "trailer"
                | "te"
                | "proxy-authenticate"
                | "proxy-authorization"
        ) || name.as_str().starts_with("access-control-")
        {
            continue;
        }
        response = response.header(name, HeaderValue::try_from(value)?);
    }
    // HEAD advertises the size of the corresponding GET representation.
    if head && web.status != 204 && web.status != 304 {
        response = response.header(header::CONTENT_LENGTH, web.body.len());
    }
    let no_body = head || web.status == 204 || web.status == 304;
    Ok(response.body(if no_body {
        Body::empty()
    } else {
        Body::from(web.body)
    })?)
}
