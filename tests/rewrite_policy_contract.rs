use kse_static_rewrite_proxy::rewrite::{RewriteDecision, RewritePolicy, RewriteProfile};

#[test]
fn rewrites_only_prefixed_text_assets_for_enabled_v3_extensions() {
    let policy =
        RewritePolicy::for_allowlisted_extensions("/regions/region:shenzhen", ["ks-console-embed"]);

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
    let policy = RewritePolicy::for_allowlisted_extensions(
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

    let disabled =
        RewritePolicy::for_allowlisted_extensions("/regions/region:shenzhen", ["ks-console-embed"]);
    assert_eq!(
        disabled.decide(
            "GET",
            "/regions/region:shenzhen/jsbundles/observability/dist/observability/index.js",
        ),
        RewriteDecision::Bypass
    );
}

#[test]
fn wildcard_enables_both_rewrite_profiles_for_safe_extension_names() {
    let policy = RewritePolicy::for_all_extensions("/regions/region:shenzhen");

    for (path, expected_profile) in [
        (
            "/regions/region:shenzhen/extensions-static/kubeeye/dist/v3dist/main.js",
            RewriteProfile::ConsoleV3,
        ),
        (
            "/regions/region:shenzhen/jsbundles/observability/dist/observability/index.js",
            RewriteProfile::JsBundle,
        ),
    ] {
        assert!(matches!(
            policy.decide("GET", path),
            RewriteDecision::Rewrite { profile, .. } if profile == expected_profile
        ));
    }
}

#[test]
fn wildcard_rejects_unsafe_extension_names_from_request_paths() {
    let policy = RewritePolicy::for_all_extensions("/regions/region:shenzhen");
    let too_long = "a".repeat(129);

    for extension in [".hidden", "bad%2Fname", "bad:name", too_long.as_str()] {
        for path in [
            format!("/regions/region:shenzhen/extensions-static/{extension}/dist/v3dist/main.js"),
            format!("/regions/region:shenzhen/jsbundles/{extension}/dist/{extension}/index.js"),
        ] {
            assert_eq!(
                policy.decide("GET", &path),
                RewriteDecision::Bypass,
                "unsafe extension should bypass: {extension:?}"
            );
        }
    }
}

#[test]
fn wildcard_collapses_the_extension_metrics_label() {
    let wildcard = RewritePolicy::for_all_extensions("/regions/region:shenzhen");
    let allowlist =
        RewritePolicy::for_allowlisted_extensions("/regions/region:shenzhen", ["kubeeye"]);

    assert_eq!(wildcard.metrics_extension_label("kubeeye"), "*");
    assert_eq!(allowlist.metrics_extension_label("kubeeye"), "kubeeye");
}

#[test]
fn existing_constructor_only_interprets_standalone_wildcard_as_match_all() {
    let wildcard = RewritePolicy::new("/regions/region:shenzhen", ["*"]);
    assert!(matches!(
        wildcard.decide(
            "GET",
            "/regions/region:shenzhen/extensions-static/observability/dist/v3dist/main.js"
        ),
        RewriteDecision::Rewrite { .. }
    ));

    let policy = RewritePolicy::new("/regions/region:shenzhen", ["*", "kubeeye"]);
    assert!(matches!(
        policy.decide(
            "GET",
            "/regions/region:shenzhen/extensions-static/kubeeye/dist/v3dist/main.js"
        ),
        RewriteDecision::Rewrite { .. }
    ));
    assert_eq!(
        policy.decide(
            "GET",
            "/regions/region:shenzhen/extensions-static/observability/dist/v3dist/main.js"
        ),
        RewriteDecision::Bypass
    );
    assert_eq!(policy.metrics_extension_label("kubeeye"), "kubeeye");
}
