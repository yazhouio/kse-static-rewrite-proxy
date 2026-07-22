use std::collections::HashSet;
use std::net::SocketAddr;

use percent_encoding::percent_decode_str;
use serde::Deserialize;
use thiserror::Error;
use url::Url;

const MAX_BASE_PATH_LENGTH: usize = 1024;
const DEFAULT_MAX_DECODED_BYTES: usize = 20 * 1024 * 1024;
const DEFAULT_MAX_CONCURRENT: usize = 4;
const DEFAULT_MAX_QUEUED: usize = 32;

#[derive(Debug, Clone)]
pub struct EffectiveConfig {
    base_path: String,
    listen: SocketAddr,
    admin_listen: SocketAddr,
    upstream: SocketAddr,
    enabled_extensions: Vec<String>,
    max_decoded_bytes: usize,
    max_concurrent: usize,
    max_queued: usize,
}

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("configuration is not valid YAML: {0}")]
    InvalidYaml(#[from] yaml_serde::Error),
    #[error("client.basePath {0}")]
    InvalidBasePath(String),
    #[error("rewriteSidecar.listen must be a socket address: {0}")]
    InvalidListen(String),
    #[error("rewriteSidecar.adminListen must be a socket address: {0}")]
    InvalidAdminListen(String),
    #[error("rewriteSidecar.adminListen must differ from rewriteSidecar.listen")]
    ConflictingListeners,
    #[error("rewriteSidecar.{0} overlaps rewriteSidecar.upstream")]
    ListenerOverlapsUpstream(&'static str),
    #[error("rewriteSidecar.upstream {0}")]
    InvalidUpstream(String),
    #[error("rewriteSidecar.rewrite.enabledExtensions contains invalid extension name: {0}")]
    InvalidExtension(String),
    #[error("rewriteSidecar.rewrite.enabledExtensions contains duplicate extension name: {0}")]
    DuplicateExtension(String),
    #[error("rewriteSidecar.rewrite.{0} must be greater than zero")]
    NonPositiveLimit(&'static str),
}

#[derive(Debug, Deserialize)]
struct RawConfig {
    #[serde(default)]
    client: RawClientConfig,
    #[serde(rename = "rewriteSidecar")]
    sidecar: RawSidecarConfig,
}

#[derive(Debug, Default, Deserialize)]
struct RawClientConfig {
    #[serde(default, rename = "basePath")]
    base_path: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSidecarConfig {
    listen: String,
    #[serde(default = "default_admin_listen", rename = "adminListen")]
    admin_listen: String,
    upstream: String,
    #[serde(default)]
    rewrite: RawRewriteConfig,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawRewriteConfig {
    #[serde(rename = "enabledExtensions")]
    enabled_extensions: Vec<String>,
    #[serde(rename = "maxDecodedBytes")]
    max_decoded_bytes: usize,
    #[serde(rename = "maxConcurrent")]
    max_concurrent: usize,
    #[serde(rename = "maxQueued")]
    max_queued: usize,
}

impl Default for RawRewriteConfig {
    fn default() -> Self {
        Self {
            enabled_extensions: Vec::new(),
            max_decoded_bytes: DEFAULT_MAX_DECODED_BYTES,
            max_concurrent: DEFAULT_MAX_CONCURRENT,
            max_queued: DEFAULT_MAX_QUEUED,
        }
    }
}

impl EffectiveConfig {
    pub fn from_yaml(input: &str) -> Result<Self, ConfigError> {
        let raw: RawConfig = yaml_serde::from_str(input)?;
        let base_path = normalize_base_path(&raw.client.base_path)?;
        let listen = raw
            .sidecar
            .listen
            .parse()
            .map_err(|_| ConfigError::InvalidListen(raw.sidecar.listen.clone()))?;
        let admin_listen = raw
            .sidecar
            .admin_listen
            .parse()
            .map_err(|_| ConfigError::InvalidAdminListen(raw.sidecar.admin_listen.clone()))?;
        if socket_addrs_overlap(listen, admin_listen) {
            return Err(ConfigError::ConflictingListeners);
        }
        let upstream = parse_upstream(&raw.sidecar.upstream)?;
        if socket_addrs_overlap(listen, upstream) {
            return Err(ConfigError::ListenerOverlapsUpstream("listen"));
        }
        if socket_addrs_overlap(admin_listen, upstream) {
            return Err(ConfigError::ListenerOverlapsUpstream("adminListen"));
        }
        let rewrite = raw.sidecar.rewrite;

        if rewrite.max_decoded_bytes == 0 {
            return Err(ConfigError::NonPositiveLimit("maxDecodedBytes"));
        }
        if rewrite.max_concurrent == 0 {
            return Err(ConfigError::NonPositiveLimit("maxConcurrent"));
        }

        let mut seen = HashSet::new();
        for extension in &rewrite.enabled_extensions {
            if !is_safe_extension_name(extension) {
                return Err(ConfigError::InvalidExtension(extension.clone()));
            }
            if !seen.insert(extension.clone()) {
                return Err(ConfigError::DuplicateExtension(extension.clone()));
            }
        }

        Ok(Self {
            base_path,
            listen,
            admin_listen,
            upstream,
            enabled_extensions: rewrite.enabled_extensions,
            max_decoded_bytes: rewrite.max_decoded_bytes,
            max_concurrent: rewrite.max_concurrent,
            max_queued: rewrite.max_queued,
        })
    }

    pub fn base_path(&self) -> &str {
        &self.base_path
    }

    pub fn listen(&self) -> SocketAddr {
        self.listen
    }

    pub fn admin_listen(&self) -> SocketAddr {
        self.admin_listen
    }

    pub fn upstream(&self) -> SocketAddr {
        self.upstream
    }

    pub fn enabled_extensions(&self) -> &[String] {
        &self.enabled_extensions
    }

    pub fn max_decoded_bytes(&self) -> usize {
        self.max_decoded_bytes
    }

    pub fn max_concurrent(&self) -> usize {
        self.max_concurrent
    }

    pub fn max_queued(&self) -> usize {
        self.max_queued
    }
}

fn default_admin_listen() -> String {
    "0.0.0.0:9090".to_owned()
}

fn socket_addrs_overlap(first: SocketAddr, second: SocketAddr) -> bool {
    first.port() == second.port()
        && (first.ip().is_unspecified()
            || second.ip().is_unspecified()
            || first.ip() == second.ip())
}

fn parse_upstream(value: &str) -> Result<SocketAddr, ConfigError> {
    let parsed = Url::parse(value)
        .map_err(|_| ConfigError::InvalidUpstream("must be an absolute HTTP URL".into()))?;
    if parsed.scheme() != "http" {
        return Err(ConfigError::InvalidUpstream(
            "must use http inside the Pod".into(),
        ));
    }
    if parsed.username() != ""
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.path() != "/"
    {
        return Err(ConfigError::InvalidUpstream(
            "must contain only a loopback host and port".into(),
        ));
    }

    let host = parsed
        .host_str()
        .ok_or_else(|| ConfigError::InvalidUpstream("must include a host".into()))?;
    if host != "127.0.0.1" && host != "::1" && host != "localhost" {
        return Err(ConfigError::InvalidUpstream(
            "must use a loopback host".into(),
        ));
    }
    let port = parsed
        .port()
        .ok_or_else(|| ConfigError::InvalidUpstream("must include an explicit port".into()))?;
    if host == "localhost" {
        return Ok(SocketAddr::from(([127, 0, 0, 1], port)));
    }
    format!("{host}:{port}")
        .parse()
        .or_else(|_| format!("[::1]:{port}").parse())
        .map_err(|_| ConfigError::InvalidUpstream("contains an invalid address".into()))
}

fn normalize_base_path(input: &str) -> Result<String, ConfigError> {
    if input.is_empty() || input == "/" {
        return Ok(String::new());
    }
    if input.chars().count() > MAX_BASE_PATH_LENGTH {
        return Err(invalid_base_path("must not exceed 1024 characters"));
    }
    if !input.starts_with('/') || input.starts_with("//") {
        return Err(invalid_base_path("must be an absolute same-origin path"));
    }
    if contains_forbidden_character(input) {
        return Err(invalid_base_path(
            "must not contain query, hash, quotes, backslash, line separators, or control characters",
        ));
    }
    validate_percent_encoding(input)?;

    let decoded = decode_repeatedly(input)?;
    if contains_forbidden_character(&decoded)
        || decoded.contains("//")
        || has_dot_segment(input)
        || has_dot_segment(&decoded)
    {
        return Err(invalid_base_path(
            "contains unsafe encoded characters or path segments",
        ));
    }

    let base = Url::parse("http://kubesphere.local").expect("static base URL is valid");
    let parsed = base
        .join(input)
        .map_err(|_| invalid_base_path("must be an absolute same-origin path"))?;
    if parsed.origin() != base.origin() || parsed.path() != input {
        return Err(invalid_base_path("must not change when URL-normalized"));
    }

    Ok(input.trim_end_matches('/').to_string())
}

fn invalid_base_path(message: &str) -> ConfigError {
    ConfigError::InvalidBasePath(message.to_string())
}

fn contains_forbidden_character(value: &str) -> bool {
    value.chars().any(|character| {
        matches!(
            character,
            '?' | '#' | '\'' | '"' | '\\' | '\u{2028}' | '\u{2029}'
        ) || character.is_control()
    })
}

fn has_dot_segment(value: &str) -> bool {
    value
        .split('/')
        .any(|segment| segment == "." || segment == "..")
}

fn validate_percent_encoding(value: &str) -> Result<(), ConfigError> {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit()
            {
                return Err(invalid_base_path("contains invalid percent encoding"));
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    Ok(())
}

fn decode_repeatedly(value: &str) -> Result<String, ConfigError> {
    let mut decoded = value.to_string();
    for _ in 0..MAX_BASE_PATH_LENGTH {
        validate_percent_encoding(&decoded)?;
        let next = percent_decode_str(&decoded)
            .decode_utf8()
            .map_err(|_| invalid_base_path("contains invalid percent encoding"))?
            .into_owned();
        if next == decoded {
            return Ok(decoded);
        }
        decoded = next;
    }
    Ok(decoded)
}

fn is_safe_extension_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        && value.as_bytes()[0].is_ascii_alphanumeric()
}
