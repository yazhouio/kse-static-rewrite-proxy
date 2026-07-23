use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bytes::Bytes;
use http::header::{
    ACCEPT_ENCODING, ACCEPT_RANGES, CACHE_CONTROL, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE,
    ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, IF_RANGE, LAST_MODIFIED, RANGE, VARY,
};
use http::{HeaderValue, StatusCode};
use pingora::http::{RequestHeader, ResponseHeader};
use pingora::modules::http::HttpModules;
use pingora::modules::http::compression::{ResponseCompression, ResponseCompressionBuilder};
use pingora::proxy::{ProxyHttp, Session};
use pingora::upstreams::peer::HttpPeer;
use pingora::{Error, ErrorType, Result};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tracing::{error, info};

use crate::admin::AdminApp;
use crate::config::EffectiveConfig;
use crate::literal::StreamingRewritePipeline;
use crate::metrics::Metrics;
use crate::rewrite::{
    REWRITE_RULE_VERSION, RewriteDecision, RewritePolicy, build_response_rewriter,
};

const COMPRESSION_LEVEL: u32 = 6;
const UPSTREAM_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

static REQUEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone)]
pub struct KseRewriteProxy {
    config: Arc<EffectiveConfig>,
    policy: RewritePolicy,
    admission: Admission,
    metrics: Metrics,
}

pub struct RequestContext {
    started: Instant,
    request_id: String,
    method: String,
    path: String,
    extension: Option<String>,
    decision: RewriteDecision,
    rewriter: Option<StreamingRewritePipeline>,
    active_permit: Option<OwnedSemaphorePermit>,
    client_if_none_match: Option<HeaderValue>,
    suppress_body: bool,
    input_bytes: usize,
    output_bytes: usize,
    outcome: &'static str,
}

#[derive(Clone)]
struct Admission {
    active: Arc<Semaphore>,
    queued: Arc<Semaphore>,
}

impl KseRewriteProxy {
    pub fn new(config: EffectiveConfig) -> Result<Self, prometheus::Error> {
        let metrics = Metrics::new()?;
        let policy = RewritePolicy::new(config.base_path(), config.enabled_extensions());
        let admission = Admission {
            active: Arc::new(Semaphore::new(config.max_concurrent())),
            queued: Arc::new(Semaphore::new(config.max_queued())),
        };
        Ok(Self {
            config: Arc::new(config),
            policy,
            admission,
            metrics,
        })
    }

    pub fn admin_app(&self) -> AdminApp {
        AdminApp::new(self.config.upstream(), self.metrics.clone())
    }

    fn disable_compression(session: &mut Session) {
        if let Some(compression) = session
            .downstream_modules_ctx
            .get_mut::<ResponseCompression>()
        {
            compression.adjust_level(0);
        }
    }

    async fn admit_rewrite(&self, session: &mut Session, ctx: &mut RequestContext) -> Result<bool> {
        if let Ok(permit) = self.admission.active.clone().try_acquire_owned() {
            self.metrics.active.inc();
            ctx.active_permit = Some(permit);
            return Ok(true);
        }

        let Ok(queue_permit) = self.admission.queued.clone().try_acquire_owned() else {
            Self::disable_compression(session);
            ctx.outcome = "queue_full";
            write_response(
                session,
                StatusCode::SERVICE_UNAVAILABLE,
                "text/plain; charset=utf-8",
                b"rewrite queue is full\n".to_vec(),
                Some(("Retry-After", "1")),
            )
            .await?;
            return Ok(false);
        };

        self.metrics.queued.inc();
        ctx.outcome = "queued";
        let active_permit = self
            .admission
            .active
            .clone()
            .acquire_owned()
            .await
            .map_err(|cause| {
                Error::because(ErrorType::InternalError, "rewrite admission closed", cause)
            })?;
        drop(queue_permit);
        self.metrics.queued.dec();
        self.metrics.active.inc();
        ctx.active_permit = Some(active_permit);
        Ok(true)
    }

    fn stop_rewrite(&self, session: &mut Session, ctx: &mut RequestContext, outcome: &'static str) {
        Self::disable_compression(session);
        ctx.rewriter = None;
        ctx.outcome = outcome;
        if ctx.active_permit.take().is_some() {
            self.metrics.active.dec();
        }
    }
}

#[async_trait]
impl ProxyHttp for KseRewriteProxy {
    type CTX = RequestContext;

    fn new_ctx(&self) -> Self::CTX {
        RequestContext {
            started: Instant::now(),
            request_id: new_request_id(),
            method: String::new(),
            path: String::new(),
            extension: None,
            decision: RewriteDecision::Bypass,
            rewriter: None,
            active_permit: None,
            client_if_none_match: None,
            suppress_body: false,
            input_bytes: 0,
            output_bytes: 0,
            outcome: "bypass",
        }
    }

    fn init_downstream_modules(&self, modules: &mut HttpModules) {
        modules.add_module(ResponseCompressionBuilder::enable(COMPRESSION_LEVEL));
    }

    async fn request_filter(&self, session: &mut Session, ctx: &mut Self::CTX) -> Result<bool> {
        let request = session.req_header();
        ctx.method = request.method.as_str().to_owned();
        ctx.path = request.uri.path().to_owned();
        if let Some(request_id) = request
            .headers
            .get("x-request-id")
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.is_empty() && value.len() <= 128)
        {
            ctx.request_id = request_id.to_owned();
        }
        ctx.client_if_none_match = request.headers.get(IF_NONE_MATCH).cloned();

        ctx.decision = self.policy.decide(&ctx.method, &ctx.path);
        let RewriteDecision::Rewrite {
            extension,
            source,
            replacement,
            head_only,
        } = ctx.decision.clone()
        else {
            Self::disable_compression(session);
            return Ok(false);
        };

        ctx.extension = Some(extension);
        ctx.outcome = "rewrite";
        if !head_only {
            if !self.admit_rewrite(session, ctx).await? {
                return Ok(true);
            }
            ctx.rewriter = Some(
                build_response_rewriter(
                    self.config.base_path(),
                    &source,
                    &replacement,
                    self.config.max_decoded_bytes(),
                )
                .map_err(|cause| {
                    Error::because(
                        ErrorType::InternalError,
                        "failed to initialize rewrite pipeline",
                        cause,
                    )
                })?,
            );
        }
        Ok(false)
    }

    async fn upstream_peer(
        &self,
        _session: &mut Session,
        _ctx: &mut Self::CTX,
    ) -> Result<Box<HttpPeer>> {
        let mut peer = HttpPeer::new(self.config.upstream(), false, "localhost".to_owned());
        peer.options.connection_timeout = Some(UPSTREAM_CONNECT_TIMEOUT);
        peer.options.total_connection_timeout = Some(UPSTREAM_CONNECT_TIMEOUT);
        Ok(Box::new(peer))
    }

    async fn upstream_request_filter(
        &self,
        _session: &mut Session,
        upstream_request: &mut RequestHeader,
        ctx: &mut Self::CTX,
    ) -> Result<()> {
        upstream_request.insert_header("x-request-id", ctx.request_id.clone())?;
        if matches!(ctx.decision, RewriteDecision::Rewrite { .. }) {
            upstream_request.insert_header(ACCEPT_ENCODING, "identity")?;
            upstream_request.remove_header(&IF_NONE_MATCH);
            upstream_request.remove_header(&IF_MODIFIED_SINCE);
            upstream_request.remove_header(&RANGE);
            upstream_request.remove_header(&IF_RANGE);
        }
        Ok(())
    }

    async fn response_filter(
        &self,
        session: &mut Session,
        upstream_response: &mut ResponseHeader,
        ctx: &mut Self::CTX,
    ) -> Result<()> {
        upstream_response.insert_header("x-request-id", ctx.request_id.clone())?;
        if !matches!(ctx.decision, RewriteDecision::Rewrite { .. }) {
            return Ok(());
        }

        if upstream_response.status != StatusCode::OK {
            self.stop_rewrite(session, ctx, "upstream_status_bypass");
            return Ok(());
        }
        if !is_rewritable_content_type(upstream_response.headers.get(CONTENT_TYPE)) {
            self.stop_rewrite(session, ctx, "invalid_content_type");
            return Error::e_explain(
                ErrorType::HTTPStatus(502),
                "target rewrite response has a non-text Content-Type",
            );
        }
        if !is_identity_encoding(upstream_response.headers.get(CONTENT_ENCODING)) {
            self.stop_rewrite(session, ctx, "unexpected_content_encoding");
            return Error::e_explain(
                ErrorType::HTTPStatus(502),
                "target rewrite response was not decoded by the upstream",
            );
        }

        let derived_etag = upstream_response
            .headers
            .get(ETAG)
            .and_then(reliable_etag)
            .map(|etag| {
                derive_etag(
                    etag,
                    self.config.base_path(),
                    ctx.extension.as_deref().unwrap_or(""),
                )
            });
        for header in [
            CONTENT_LENGTH.as_str(),
            CONTENT_ENCODING.as_str(),
            "content-md5",
            "content-digest",
            "repr-digest",
            "digest",
            "content-range",
            LAST_MODIFIED.as_str(),
            ACCEPT_RANGES.as_str(),
        ] {
            upstream_response.remove_header(header);
        }
        upstream_response.insert_header(ACCEPT_RANGES, "none")?;
        ensure_vary_accept_encoding(upstream_response)?;

        if let Some(etag) = derived_etag {
            upstream_response.insert_header(ETAG, etag.clone())?;
            if if_none_match_matches(ctx.client_if_none_match.as_ref(), &etag) {
                upstream_response.status = StatusCode::NOT_MODIFIED;
                upstream_response.remove_header(&CONTENT_TYPE);
                ctx.suppress_body = true;
                ctx.rewriter = None;
                self.stop_rewrite(session, ctx, "not_modified");
            }
        } else {
            upstream_response.remove_header(&ETAG);
            upstream_response.insert_header(CACHE_CONTROL, "no-store")?;
        }
        Ok(())
    }

    fn response_body_filter(
        &self,
        _session: &mut Session,
        body: &mut Option<Bytes>,
        end_of_stream: bool,
        ctx: &mut Self::CTX,
    ) -> Result<Option<Duration>> {
        if ctx.suppress_body {
            *body = None;
            return Ok(None);
        }
        let Some(rewriter) = ctx.rewriter.as_mut() else {
            return Ok(None);
        };

        let input = body.take().unwrap_or_default();
        ctx.input_bytes += input.len();
        let mut output = rewriter.push(&input).map_err(|cause| {
            Error::because(ErrorType::HTTPStatus(502), "response rewrite failed", cause)
        })?;
        if end_of_stream {
            output.extend(rewriter.finish().map_err(|cause| {
                Error::because(ErrorType::HTTPStatus(502), "response rewrite failed", cause)
            })?);
        }
        ctx.output_bytes += output.len();
        *body = (!output.is_empty()).then(|| Bytes::from(output));
        Ok(None)
    }

    async fn logging(
        &self,
        _session: &mut Session,
        error_value: Option<&Error>,
        ctx: &mut Self::CTX,
    ) {
        if ctx.active_permit.take().is_some() {
            self.metrics.active.dec();
        }
        let extension = ctx.extension.as_deref().unwrap_or("none");
        let outcome = if error_value.is_some() {
            "error"
        } else {
            ctx.outcome
        };
        self.metrics
            .requests
            .with_label_values(&[outcome, extension])
            .inc();
        self.metrics
            .duration
            .with_label_values(&[outcome])
            .observe(ctx.started.elapsed().as_secs_f64());
        if ctx.input_bytes > 0 {
            self.metrics
                .rewrite_input_bytes
                .with_label_values(&[extension])
                .inc_by(ctx.input_bytes as u64);
            self.metrics
                .rewrite_output_bytes
                .with_label_values(&[extension])
                .inc_by(ctx.output_bytes as u64);
        }

        if let Some(proxy_error) = error_value {
            error!(
                request_id = %ctx.request_id,
                method = %ctx.method,
                path = %ctx.path,
                extension,
                outcome,
                elapsed_ms = ctx.started.elapsed().as_millis() as u64,
                error = %proxy_error,
                "request completed"
            );
        } else {
            info!(
                request_id = %ctx.request_id,
                method = %ctx.method,
                path = %ctx.path,
                extension,
                outcome,
                elapsed_ms = ctx.started.elapsed().as_millis() as u64,
                "request completed"
            );
        }
    }
}

async fn write_response(
    session: &mut Session,
    status: StatusCode,
    content_type: &str,
    body: Vec<u8>,
    extra_header: Option<(&'static str, &'static str)>,
) -> Result<()> {
    let mut response = ResponseHeader::build(status, Some(4))?;
    response.insert_header(CONTENT_TYPE, content_type)?;
    response.insert_header(CONTENT_LENGTH, body.len().to_string())?;
    if let Some((name, value)) = extra_header {
        response.insert_header(name, value)?;
    }
    let end_of_stream = body.is_empty();
    session
        .write_response_header(Box::new(response), end_of_stream)
        .await?;
    if !end_of_stream {
        session
            .write_response_body(Some(Bytes::from(body)), true)
            .await?;
    }
    Ok(())
}

fn is_rewritable_content_type(value: Option<&HeaderValue>) -> bool {
    let Some(value) = value.and_then(|header| header.to_str().ok()) else {
        return false;
    };
    let mut parts = value.split(';').map(str::trim);
    let media_type = parts.next().unwrap_or_default().to_ascii_lowercase();
    let media_type_allowed = matches!(
        media_type.as_str(),
        "text/javascript"
            | "application/javascript"
            | "application/x-javascript"
            | "text/css"
            | "application/json"
            | "text/json"
            | "text/html"
            | "application/xhtml+xml"
    );
    media_type_allowed
        && parts.all(|parameter| {
            let Some((name, value)) = parameter.split_once('=') else {
                return true;
            };
            !name.trim().eq_ignore_ascii_case("charset")
                || matches!(
                    value.trim().trim_matches('"').to_ascii_lowercase().as_str(),
                    "utf-8" | "utf8"
                )
        })
}

fn is_identity_encoding(value: Option<&HeaderValue>) -> bool {
    value
        .and_then(|header| header.to_str().ok())
        .is_none_or(|encoding| {
            encoding.trim().is_empty() || encoding.eq_ignore_ascii_case("identity")
        })
}

fn derive_etag(upstream_etag: &[u8], base_path: &str, extension: &str) -> String {
    let mut hasher = blake3::Hasher::new();
    for component in [
        upstream_etag,
        base_path.as_bytes(),
        extension.as_bytes(),
        REWRITE_RULE_VERSION.as_bytes(),
    ] {
        hasher.update(component);
        hasher.update(&[0]);
    }
    let digest = URL_SAFE_NO_PAD.encode(hasher.finalize().as_bytes());
    format!("W/\"kserw-{REWRITE_RULE_VERSION}-{digest}\"")
}

fn reliable_etag(value: &HeaderValue) -> Option<&[u8]> {
    let bytes = value.as_bytes();
    let opaque = bytes.strip_prefix(b"W/").unwrap_or(bytes);
    if opaque.len() < 2 || opaque.first() != Some(&b'"') || opaque.last() != Some(&b'"') {
        return None;
    }
    opaque[1..opaque.len() - 1]
        .iter()
        .all(|byte| *byte == 0x21 || (0x23..=0x7e).contains(byte) || *byte >= 0x80)
        .then_some(bytes)
}

fn if_none_match_matches(value: Option<&HeaderValue>, derived_etag: &str) -> bool {
    value
        .and_then(|header| header.to_str().ok())
        .is_some_and(|header| {
            header
                .split(',')
                .map(str::trim)
                .any(|candidate| candidate == "*" || candidate == derived_etag)
        })
}

fn ensure_vary_accept_encoding(response: &mut ResponseHeader) -> Result<()> {
    let vary = response
        .headers
        .get_all(VARY)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if vary
        .iter()
        .any(|value| value == "*" || value.eq_ignore_ascii_case("accept-encoding"))
    {
        return Ok(());
    }
    let mut updated = vary;
    updated.push("Accept-Encoding".to_owned());
    response.insert_header(VARY, updated.join(", "))
}

fn new_request_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("kserw-{nanos:x}-{sequence:x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_type_allows_only_supported_utf8_text() {
        assert!(is_rewritable_content_type(Some(&HeaderValue::from_static(
            "application/javascript; charset=UTF-8"
        ))));
        assert!(is_rewritable_content_type(Some(&HeaderValue::from_static(
            "text/css"
        ))));
        assert!(!is_rewritable_content_type(Some(
            &HeaderValue::from_static("font/woff2")
        )));
        assert!(!is_rewritable_content_type(Some(
            &HeaderValue::from_static("text/html; charset=gbk")
        )));
    }

    #[test]
    fn derived_etag_changes_with_base_path() {
        let first = derive_etag(b"\"upstream\"", "/regions/region:shenzhen", "embed");
        let second = derive_etag(b"\"upstream\"", "/regions/region:beijing", "embed");
        assert_ne!(first, second);
        assert!(first.starts_with("W/\"kserw-v11-"));
    }

    #[test]
    fn only_uses_well_formed_upstream_etags() {
        assert_eq!(
            reliable_etag(&HeaderValue::from_static("\"asset-v1\"")),
            Some(b"\"asset-v1\"".as_slice())
        );
        assert_eq!(
            reliable_etag(&HeaderValue::from_static("W/\"asset-v1\"")),
            Some(b"W/\"asset-v1\"".as_slice())
        );
        assert_eq!(reliable_etag(&HeaderValue::from_static("asset-v1")), None);
        assert_eq!(reliable_etag(&HeaderValue::from_static("")), None);
    }
}
