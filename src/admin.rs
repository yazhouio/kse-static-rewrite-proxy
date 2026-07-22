use std::net::SocketAddr;
use std::time::Duration;

use async_trait::async_trait;
use http::{Response, StatusCode};
use pingora::apps::http_app::ServeHttp;
use pingora::protocols::http::ServerSession;

use crate::metrics::Metrics;

const READINESS_TIMEOUT: Duration = Duration::from_millis(250);

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

fn response(status: StatusCode, content_type: &str, body: Vec<u8>) -> Response<Vec<u8>> {
    Response::builder()
        .status(status)
        .header("content-type", content_type)
        .header("content-length", body.len())
        .body(body)
        .expect("admin response uses valid static headers")
}
