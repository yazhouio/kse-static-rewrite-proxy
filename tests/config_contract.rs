use kse_static_rewrite_proxy::config::EffectiveConfig;

#[test]
fn loads_base_path_and_rewrite_limits_from_the_shared_console_config() {
    let config = EffectiveConfig::from_yaml(
        r#"
client:
  basePath: /regions/region:shenzhen/
rewriteSidecar:
  listen: 0.0.0.0:8080
  adminListen: 0.0.0.0:9090
  upstream: http://127.0.0.1:8000
  rewrite:
    enabledExtensions:
      - ks-console-embed
    maxDecodedBytes: 20971520
    maxConcurrent: 4
    maxQueued: 32
"#,
    )
    .expect("valid shared config");

    assert_eq!(config.base_path(), "/regions/region:shenzhen");
    assert_eq!(config.listen().to_string(), "0.0.0.0:8080");
    assert_eq!(config.admin_listen().to_string(), "0.0.0.0:9090");
    assert_eq!(config.upstream().to_string(), "127.0.0.1:8000");
    assert_eq!(config.enabled_extensions(), ["ks-console-embed"]);
    assert_eq!(config.max_decoded_bytes(), 20_971_520);
    assert_eq!(config.max_concurrent(), 4);
    assert_eq!(config.max_queued(), 32);
}

#[test]
fn rejects_overlapping_proxy_admin_and_upstream_sockets() {
    for (listen, admin_listen, upstream) in [
        ("0.0.0.0:8080", "127.0.0.1:8080", "http://127.0.0.1:8000"),
        ("0.0.0.0:8000", "0.0.0.0:9090", "http://127.0.0.1:8000"),
        ("0.0.0.0:8080", "[::]:8000", "http://127.0.0.1:8000"),
    ] {
        let yaml = format!(
            r#"
client:
  basePath: /regions/region:shenzhen
rewriteSidecar:
  listen: {listen}
  adminListen: {admin_listen}
  upstream: {upstream}
  rewrite:
    enabledExtensions: [ks-console-embed]
"#
        );
        assert!(
            EffectiveConfig::from_yaml(&yaml).is_err(),
            "overlap should fail: {listen}, {admin_listen}, {upstream}"
        );
    }
}

#[test]
fn rejects_base_paths_that_could_break_javascript_string_literals() {
    for base_path in [
        "/regions/region:shen'zhen",
        "/regions/region:\"shenzhen",
        "/regions/region:shen\u{2028}zhen",
    ] {
        let yaml = format!(
            r#"
client:
  basePath: |-
    {base_path}
rewriteSidecar:
  listen: 0.0.0.0:8080
  adminListen: 0.0.0.0:9090
  upstream: http://127.0.0.1:8000
  rewrite:
    enabledExtensions: [ks-console-embed]
"#
        );
        assert!(
            EffectiveConfig::from_yaml(&yaml).is_err(),
            "unsafe base path should fail: {base_path:?}"
        );
    }
}
