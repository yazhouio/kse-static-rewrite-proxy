use kse_static_rewrite_proxy::rewrite::{RewriteDecision, RewritePolicy};

#[test]
fn rewrites_only_prefixed_text_assets_for_enabled_v3_extensions() {
    let policy = RewritePolicy::new("/regions/region:shenzhen", ["ks-console-embed"]);

    let target = policy.decide(
        "GET",
        "/regions/region:shenzhen/extensions-static/ks-console-embed/dist/v3dist/main.js",
    );
    assert!(matches!(target, RewriteDecision::Rewrite { .. }));

    for (method, path) in [
        (
            "GET",
            "/extensions-static/ks-console-embed/dist/v3dist/main.js",
        ),
        (
            "GET",
            "/regions/region:shenzhen/extensions-static/another-extension/dist/v3dist/main.js",
        ),
        (
            "GET",
            "/regions/region:shenzhen/extensions-static/ks-console-embed/dist/main.js",
        ),
        (
            "GET",
            "/regions/region:shenzhen/extensions-static/ks-console-embed/dist/v3dist/font.woff2",
        ),
        (
            "POST",
            "/regions/region:shenzhen/extensions-static/ks-console-embed/dist/v3dist/main.js",
        ),
    ] {
        assert_eq!(policy.decide(method, path), RewriteDecision::Bypass);
    }
}
