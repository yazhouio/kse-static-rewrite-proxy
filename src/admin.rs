use std::net::SocketAddr;
use std::time::Duration;

use async_trait::async_trait;
use http::{Response, StatusCode};
use pingora::apps::http_app::ServeHttp;
use pingora::protocols::http::ServerSession;

use crate::metrics::Metrics;
use crate::rewrite::REWRITE_RULE_VERSION;

const READINESS_TIMEOUT: Duration = Duration::from_millis(250);
const BUILD_GIT_COMMIT: Option<&str> = option_env!("KSE_GIT_COMMIT");

pub struct AdminApp {
    upstream: SocketAddr,
    metrics: Metrics,
}

impl AdminApp {
    pub(crate) fn new(upstream: SocketAddr, metrics: Metrics) -> Self {
        Self { upstream, metrics }
    }
}

#[async_trait]
impl ServeHttp for AdminApp {
    async fn response(&self, session: &mut ServerSession) -> Response<Vec<u8>> {
        let path = session.req_header().uri.path();
        match path {
            "/healthz" => response(
                StatusCode::OK,
                "text/plain; charset=utf-8",
                b"ok\n".to_vec(),
            ),
            "/readyz" => {
                let ready = tokio::time::timeout(
                    READINESS_TIMEOUT,
                    tokio::net::TcpStream::connect(self.upstream),
                )
                .await
                .is_ok_and(|connection| connection.is_ok());
                if ready {
                    response(
                        StatusCode::OK,
                        "text/plain; charset=utf-8",
                        b"ready\n".to_vec(),
                    )
                } else {
                    response(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "text/plain; charset=utf-8",
                        b"not ready\n".to_vec(),
                    )
                }
            }
            "/version" => version_response(),
            "/metrics" => match self.metrics.encode() {
                Ok((content_type, body)) => response(StatusCode::OK, &content_type, body),
                Err(_) => response(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "text/plain; charset=utf-8",
                    b"failed to encode metrics\n".to_vec(),
                ),
            },
            _ => response(
                StatusCode::NOT_FOUND,
                "text/plain; charset=utf-8",
                b"not found\n".to_vec(),
            ),
        }
    }
}

fn version_response() -> Response<Vec<u8>> {
    let body = format!(
        "{{\"packageVersion\":\"{}\",\"rewriteRuleVersion\":\"{}\",\"gitCommit\":\"{}\"}}\n",
        env!("CARGO_PKG_VERSION"),
        REWRITE_RULE_VERSION,
        build_git_commit(),
    )
    .into_bytes();
    Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json; charset=utf-8")
        .header("cache-control", "no-store")
        .header("content-length", body.len())
        .body(body)
        .expect("version response uses valid static headers")
}

fn build_git_commit() -> &'static str {
    normalize_git_commit(BUILD_GIT_COMMIT)
}

fn normalize_git_commit(value: Option<&str>) -> &str {
    value
        .filter(|value| {
            (7..=64).contains(&value.len()) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        .unwrap_or("unknown")
}

fn response(status: StatusCode, content_type: &str, body: Vec<u8>) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header("content-type", content_type)
        .header("content-length", body.len())
        .body(body)
        .expect("admin response uses valid static headers")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_response_reports_build_and_rule_versions_without_caching() {
        let response = version_response();
        let expected = format!(
            "{{\"packageVersion\":\"{}\",\"rewriteRuleVersion\":\"{}\",\"gitCommit\":\"{}\"}}\n",
            env!("CARGO_PKG_VERSION"),
            REWRITE_RULE_VERSION,
            build_git_commit(),
        );

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get("content-type").unwrap(),
            "application/json; charset=utf-8"
        );
        assert_eq!(response.headers().get("cache-control").unwrap(), "no-store");
        assert_eq!(response.body(), expected.as_bytes());
    }

    #[test]
    fn git_commit_requires_a_bounded_hexadecimal_sha() {
        let too_long = "a".repeat(65);
        for (value, expected) in [
            (None, "unknown"),
            (Some(""), "unknown"),
            (Some("abcdef"), "unknown"),
            (Some("abc1234"), "abc1234"),
            (
                Some("0123456789abcdef0123456789abcdef01234567"),
                "0123456789abcdef0123456789abcdef01234567",
            ),
            (Some("not-a-git-sha"), "unknown"),
            (Some(too_long.as_str()), "unknown"),
        ] {
            assert_eq!(normalize_git_commit(value), expected);
        }
    }
}
