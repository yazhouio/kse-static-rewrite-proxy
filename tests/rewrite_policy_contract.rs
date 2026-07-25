use kse_static_rewrite_proxy::rewrite::{RewriteDecision, RewritePolicy, RewriteProfile};

#[test]
fn rewrites_the_fixed_kubekey_installer_html_path() {
    let policy = RewritePolicy::for_allowlisted_extensions(
        "/regions/region:region-04",
        std::iter::empty::<&str>(),
    );

    for method in ["GET", "HEAD"] {
        assert!(matches!(
            policy.decide(
                method,
                "/regions/region:region-04/proxy/kubekey/"
            ),
            RewriteDecision::Rewrite {
                profile: RewriteProfile::Kubekey,
                ref extension,
                head_only,
            } if extension == "kubekey" && head_only == method.eq_ignore_ascii_case("HEAD")
        ));
    }

    for (method, path) in [
        ("GET", "/proxy/kubekey/"),
        ("POST", "/regions/region:region-04/proxy/kubekey/"),
    ] {
        assert_eq!(policy.decide(method, path), RewriteDecision::Bypass);
    }
}

#[test]
fn rewrites_javascript_below_named_proxy_paths() {
    let policy = RewritePolicy::for_allowlisted_extensions(
        "/regions/region:region-04",
        std::iter::empty::<&str>(),
    );

    for (method, path, expected_name) in [
        (
            "GET",
            "/regions/region:region-04/proxy/kubekey/index.js",
            "kubekey",
        ),
        (
            "HEAD",
            "/regions/region:region-04/proxy/kubekey/assets/chunks/index.js",
            "kubekey",
        ),
        (
            "GET",
            "/regions/region:region-04/proxy/another-app/dist/main.js",
            "another-app",
        ),
        (
            "GET",
            "/regions/region:region-04/proxy/app:name/dist/main.js",
            "app:name",
        ),
        (
            "GET",
            "/regions/region:region-04/proxy/app%20name/dist/main.js",
            "app%20name",
        ),
    ] {
        assert!(matches!(
            policy.decide(method, path),
            RewriteDecision::Rewrite {
                profile: RewriteProfile::ProxyJs,
                ref extension,
                head_only,
            } if extension == expected_name && head_only == method.eq_ignore_ascii_case("HEAD")
        ));
    }

    for (method, path) in [
        (
            "GET",
            "/regions/region:region-04/proxy/kubekey/assets/index.css",
        ),
        ("GET", "/proxy/kubekey/assets/index.js"),
        (
            "POST",
            "/regions/region:region-04/proxy/kubekey/assets/index.js",
        ),
        ("GET", "/regions/region:region-04/proxy//assets/index.js"),
    ] {
        assert_eq!(policy.decide(method, path), RewriteDecision::Bypass);
    }
}

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
fn disabled_extensions_override_wildcard_for_both_rewrite_profiles() {
    let policy = RewritePolicy::new_with_disabled_extensions(
        "/regions/region:shenzhen",
        ["*"],
        ["whizard-telemetry"],
    );

    for path in [
        "/regions/region:shenzhen/extensions-static/whizard-telemetry/dist/v3dist/main.js",
        "/regions/region:shenzhen/jsbundles/whizard-telemetry/dist/whizard-telemetry/index.js",
    ] {
        assert_eq!(policy.decide("GET", path), RewriteDecision::Bypass);
    }

    assert!(matches!(
        policy.decide(
            "GET",
            "/regions/region:shenzhen/extensions-static/kubeeye/dist/v3dist/main.js"
        ),
        RewriteDecision::Rewrite { .. }
    ));

    let allowlist = RewritePolicy::new_with_disabled_extensions(
        "/regions/region:shenzhen",
        ["whizard-telemetry", "kubeeye"],
        ["whizard-telemetry"],
    );
    assert_eq!(
        allowlist.decide(
            "GET",
            "/regions/region:shenzhen/extensions-static/whizard-telemetry/dist/v3dist/main.js"
        ),
        RewriteDecision::Bypass
    );
    assert!(matches!(
        allowlist.decide(
            "GET",
            "/regions/region:shenzhen/extensions-static/kubeeye/dist/v3dist/main.js"
        ),
        RewriteDecision::Rewrite { .. }
    ));
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

    assert_eq!(
        wildcard.metrics_extension_label(RewriteProfile::ConsoleV3, "kubeeye"),
        "*"
    );
    assert_eq!(
        allowlist.metrics_extension_label(RewriteProfile::ConsoleV3, "kubeeye"),
        "kubeeye"
    );
}

#[test]
fn named_proxy_metrics_use_a_bounded_profile_label() {
    let policy = RewritePolicy::for_allowlisted_extensions(
        "/regions/region:shenzhen",
        std::iter::empty::<&str>(),
    );

    assert_eq!(
        policy.metrics_extension_label(RewriteProfile::ProxyJs, "arbitrary:name"),
        "proxy-js"
    );
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
    assert_eq!(
        policy.metrics_extension_label(RewriteProfile::ConsoleV3, "kubeeye"),
        "kubeeye"
    );
}
