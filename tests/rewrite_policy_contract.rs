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
fn rewrites_only_enabled_direct_kubeeye_javascript_bundles() {
    let policy = RewritePolicy::new("/regions/region:shenzhen", ["ks-console-embed", "kubeeye"]);

    for method in ["GET", "HEAD"] {
        let target = policy.decide(
            method,
            "/regions/region:shenzhen/jsbundles/kubeeye/dist/kubeeye/index.js",
        );
        assert!(matches!(
            target,
            RewriteDecision::Rewrite {
                profile: RewriteProfile::KubeEyeJsBundle,
                ..
            }
        ));
    }

    for (method, path) in [
        ("GET", "/jsbundles/kubeeye/dist/kubeeye/index.js"),
        (
            "GET",
            "/regions/region:shenzhen/jsbundles/another-extension/dist/kubeeye/index.js",
        ),
        (
            "GET",
            "/regions/region:shenzhen/jsbundles/kubeeye/dist/kubeeye/chunks/index.js",
        ),
        (
            "GET",
            "/regions/region:shenzhen/jsbundles/kubeeye/dist/kubeeye/index.css",
        ),
        (
            "POST",
            "/regions/region:shenzhen/jsbundles/kubeeye/dist/kubeeye/index.js",
        ),
    ] {
        assert_eq!(policy.decide(method, path), RewriteDecision::Bypass);
    }

    let disabled = RewritePolicy::new("/regions/region:shenzhen", ["ks-console-embed"]);
    assert_eq!(
        disabled.decide(
            "GET",
            "/regions/region:shenzhen/jsbundles/kubeeye/dist/kubeeye/index.js",
        ),
        RewriteDecision::Bypass
    );
}
