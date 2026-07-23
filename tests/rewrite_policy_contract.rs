use kse_static_rewrite_proxy::rewrite::{RewriteDecision, RewritePolicy, RewriteProfile};

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

#[test]
fn rewrites_only_configured_direct_javascript_bundles() {
    let policy = RewritePolicy::new(
        "/regions/region:shenzhen",
        ["ks-console-embed", "observability"],
    );

    for method in ["GET", "HEAD"] {
        let target = policy.decide(
            method,
            "/regions/region:shenzhen/jsbundles/observability/dist/observability/index.js",
        );
        assert!(matches!(
            target,
            RewriteDecision::Rewrite {
                profile: RewriteProfile::JsBundle,
                ref extension,
                ..
            } if extension == "observability"
        ));
    }

    for (method, path) in [
        (
            "GET",
            "/jsbundles/observability/dist/observability/index.js",
        ),
        (
            "GET",
            "/regions/region:shenzhen/jsbundles/another-extension/dist/another-extension/index.js",
        ),
        (
            "GET",
            "/regions/region:shenzhen/jsbundles/observability/dist/another-extension/index.js",
        ),
        (
            "GET",
            "/regions/region:shenzhen/jsbundles/observability/dist/observability/chunks/index.js",
        ),
        (
            "GET",
            "/regions/region:shenzhen/jsbundles/observability/dist/observability/index.css",
        ),
        (
            "POST",
            "/regions/region:shenzhen/jsbundles/observability/dist/observability/index.js",
        ),
    ] {
        assert_eq!(policy.decide(method, path), RewriteDecision::Bypass);
    }

    let disabled = RewritePolicy::new("/regions/region:shenzhen", ["ks-console-embed"]);
    assert_eq!(
        disabled.decide(
            "GET",
            "/regions/region:shenzhen/jsbundles/observability/dist/observability/index.js",
        ),
        RewriteDecision::Bypass
    );
}
