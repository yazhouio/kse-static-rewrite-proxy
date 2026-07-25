use std::collections::HashSet;

use crate::literal::{RewriteError, StreamingRewritePipeline};

pub(crate) const REWRITE_RULE_VERSION: &str = "v26";
pub(crate) const ALL_EXTENSIONS_WILDCARD: &str = "*";
const KUBEKEY_KAPIS_PATH: &str = "/kapis/kubekey.kubesphere.io";
const KUBEKEY_PROXY_PATH: &str = "/proxy/kubekey/";
const NAMED_PROXY_ROOT: &str = "/proxy/";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RewriteProfile {
    ConsoleV3,
    FrontendIndexJsBundle,
    JsBundle,
    KubekeyAssetJs,
    NamedProxyHtml,
    ProxyJs,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RewriteDecision {
    Bypass,
    Rewrite {
        profile: RewriteProfile,
        extension: String,
        head_only: bool,
    },
}

#[derive(Debug, Clone)]
pub struct RewritePolicy {
    base_path: String,
    extensions: ExtensionMatcher,
    disabled_extensions: HashSet<String>,
}

#[derive(Debug, Clone)]
enum ExtensionMatcher {
    All,
    Allowlist(HashSet<String>),
}

impl RewritePolicy {
    pub fn new<I, S>(base_path: impl Into<String>, enabled_extensions: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self::new_with_disabled_extensions(
            base_path,
            enabled_extensions,
            std::iter::empty::<&'static str>(),
        )
    }

    pub fn new_with_disabled_extensions<EI, ES, DI, DS>(
        base_path: impl Into<String>,
        enabled_extensions: EI,
        disabled_extensions: DI,
    ) -> Self
    where
        EI: IntoIterator<Item = ES>,
        ES: AsRef<str>,
        DI: IntoIterator<Item = DS>,
        DS: AsRef<str>,
    {
        let enabled_extensions: HashSet<String> = enabled_extensions
            .into_iter()
            .map(|extension| extension.as_ref().to_string())
            .collect();
        let extensions = if enabled_extensions.len() == 1
            && enabled_extensions.contains(ALL_EXTENSIONS_WILDCARD)
        {
            ExtensionMatcher::All
        } else {
            ExtensionMatcher::Allowlist(enabled_extensions)
        };
        let disabled_extensions = disabled_extensions
            .into_iter()
            .map(|extension| extension.as_ref().to_string())
            .collect();
        Self::from_matcher(base_path, extensions, disabled_extensions)
    }

    pub fn for_all_extensions(base_path: impl Into<String>) -> Self {
        Self::new(base_path, [ALL_EXTENSIONS_WILDCARD])
    }

    pub fn for_allowlisted_extensions<I, S>(
        base_path: impl Into<String>,
        enabled_extensions: I,
    ) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self::new(base_path, enabled_extensions)
    }

    pub fn decide(&self, method: &str, path: &str) -> RewriteDecision {
        if self.base_path.is_empty()
            || (!method.eq_ignore_ascii_case("GET") && !method.eq_ignore_ascii_case("HEAD"))
        {
            return RewriteDecision::Bypass;
        }

        let named_proxy_prefix = format!("{}{NAMED_PROXY_ROOT}", self.base_path);
        if let Some(proxy_path) = path.strip_prefix(&named_proxy_prefix) {
            if let Some((name, asset_path)) = proxy_path.split_once('/')
                && !name.is_empty()
            {
                let is_javascript_asset = !asset_path.is_empty() && asset_path.ends_with(".js");
                return RewriteDecision::Rewrite {
                    profile: if is_javascript_asset
                        && name == "kubekey"
                        && asset_path.starts_with("assets/")
                    {
                        RewriteProfile::KubekeyAssetJs
                    } else if is_javascript_asset {
                        RewriteProfile::ProxyJs
                    } else {
                        RewriteProfile::NamedProxyHtml
                    },
                    extension: name.to_owned(),
                    head_only: method.eq_ignore_ascii_case("HEAD"),
                };
            }
        }

        let jsbundle_prefix = format!("{}/jsbundles/", self.base_path);
        if let Some(bundle_path) = path.strip_prefix(&jsbundle_prefix)
            && let Some((extension, distribution_path)) = bundle_path.split_once("/dist/")
            && let Some(asset_path) = distribution_path
                .strip_prefix(extension)
                .and_then(|path| path.strip_prefix('/'))
            && self.is_extension_enabled(extension)
            && !asset_path.is_empty()
            && !asset_path.contains('/')
            && asset_path.ends_with(".js")
        {
            return RewriteDecision::Rewrite {
                profile: if extension.ends_with("-frontend") && asset_path == "index.js" {
                    RewriteProfile::FrontendIndexJsBundle
                } else {
                    RewriteProfile::JsBundle
                },
                extension: extension.to_owned(),
                head_only: method.eq_ignore_ascii_case("HEAD"),
            };
        }

        let static_prefix = format!("{}/extensions-static/", self.base_path);
        let Some(extension_path) = path.strip_prefix(&static_prefix) else {
            return RewriteDecision::Bypass;
        };
        let Some((extension, asset_path)) = extension_path.split_once("/dist/v3dist/") else {
            return RewriteDecision::Bypass;
        };
        if extension.is_empty()
            || asset_path.is_empty()
            || extension.contains('/')
            || !self.is_extension_enabled(extension)
            || !is_text_asset(asset_path)
        {
            return RewriteDecision::Bypass;
        }

        RewriteDecision::Rewrite {
            profile: RewriteProfile::ConsoleV3,
            extension: extension.to_string(),
            head_only: method.eq_ignore_ascii_case("HEAD"),
        }
    }

    pub fn metrics_extension_label<'a>(
        &self,
        profile: RewriteProfile,
        extension: &'a str,
    ) -> &'a str {
        match profile {
            RewriteProfile::ProxyJs => "proxy-js",
            RewriteProfile::KubekeyAssetJs => "kubekey-assets",
            RewriteProfile::NamedProxyHtml => "proxy-html",
            RewriteProfile::ConsoleV3
            | RewriteProfile::FrontendIndexJsBundle
            | RewriteProfile::JsBundle => match self.extensions {
                ExtensionMatcher::All => ALL_EXTENSIONS_WILDCARD,
                ExtensionMatcher::Allowlist(_) => extension,
            },
        }
    }

    fn is_extension_enabled(&self, extension: &str) -> bool {
        is_safe_extension_name(extension)
            && !self.disabled_extensions.contains(extension)
            && match &self.extensions {
                ExtensionMatcher::All => true,
                ExtensionMatcher::Allowlist(enabled_extensions) => {
                    enabled_extensions.contains(extension)
                }
            }
    }

    fn from_matcher(
        base_path: impl Into<String>,
        extensions: ExtensionMatcher,
        disabled_extensions: HashSet<String>,
    ) -> Self {
        Self {
            base_path: base_path.into().trim_end_matches('/').to_string(),
            extensions,
            disabled_extensions,
        }
    }
}

pub(crate) fn is_safe_extension_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        && value.as_bytes()[0].is_ascii_alphanumeric()
}

fn is_text_asset(asset_path: &str) -> bool {
    let filename = asset_path.rsplit('/').next().unwrap_or_default();
    [".js", ".mjs", ".css", ".json", ".html", ".htm"]
        .iter()
        .any(|suffix| filename.ends_with(suffix))
}

pub(crate) fn build_selected_response_rewriter(
    profile: RewriteProfile,
    base_path: &str,
    extension: &str,
    max_bytes: usize,
) -> Result<StreamingRewritePipeline, RewriteError> {
    match profile {
        RewriteProfile::ConsoleV3 => {
            let source = format!("/extensions-static/{extension}/dist/v3dist/");
            let replacement = format!("{base_path}{source}");
            build_response_rewriter(
                base_path,
                source.as_bytes(),
                replacement.as_bytes(),
                max_bytes,
            )
        }
        RewriteProfile::FrontendIndexJsBundle | RewriteProfile::JsBundle => {
            StreamingRewritePipeline::new_with_appended_suffix(
                b"`//${window.location.host}/",
                format!("{}/", base_path.trim_start_matches('/')),
                max_bytes,
            )
        }
        RewriteProfile::KubekeyAssetJs => {
            let proxy_source = KUBEKEY_PROXY_PATH.trim_end_matches('/').as_bytes().to_vec();
            let kapis_source = KUBEKEY_KAPIS_PATH.as_bytes().to_vec();
            let proxy_replacement = [base_path.as_bytes(), proxy_source.as_slice()].concat();
            let kapis_replacement = [base_path.as_bytes(), kapis_source.as_slice()].concat();
            StreamingRewritePipeline::new(
                [
                    (proxy_source, proxy_replacement),
                    (kapis_source, kapis_replacement),
                ],
                max_bytes,
            )
        }
        RewriteProfile::ProxyJs => {
            let source = format!("{NAMED_PROXY_ROOT}{extension}");
            StreamingRewritePipeline::new(
                [(
                    source.as_bytes(),
                    format!("{base_path}{source}").into_bytes(),
                )],
                max_bytes,
            )
        }
        RewriteProfile::NamedProxyHtml => {
            let proxy_root = format!("{NAMED_PROXY_ROOT}{extension}/");
            let attribute_rules =
                [" ", "\t", "\r", "\n", "\x0C"]
                    .into_iter()
                    .flat_map(|boundary| {
                        ["href=\"", "href='", "src=\"", "src='"].map(|attribute| {
                            (
                                format!("{boundary}{attribute}{proxy_root}").into_bytes(),
                                format!("{boundary}{attribute}{base_path}{proxy_root}")
                                    .into_bytes(),
                            )
                        })
                    });
            StreamingRewritePipeline::new_with_exact(
                std::iter::empty::<(Vec<u8>, Vec<u8>)>(),
                attribute_rules,
                max_bytes,
            )
        }
    }
}

pub(crate) fn build_response_rewriter(
    base_path: &str,
    source: &[u8],
    replacement: &[u8],
    max_bytes: usize,
) -> Result<StreamingRewritePipeline, RewriteError> {
    let static_source = b"/extensions-static/".to_vec();
    let static_replacement = format!("{base_path}/extensions-static/").into_bytes();
    let exact_rules = [
        (
            b"return requestURL.replace(/\\\\/\\\\/+/, '/');".to_vec(),
            format!(
                "return requestURL.toLowerCase().startsWith('http://') || requestURL.toLowerCase().startsWith('https://') || requestURL.startsWith('//') ? requestURL : (requestURL.replace(/\\\\/\\\\/+/, '/') === '{base_path}' || requestURL.replace(/\\\\/\\\\/+/, '/').startsWith('{base_path}/') ? requestURL.replace(/\\\\/\\\\/+/, '/') : '{base_path}/'.concat(requestURL.replace(/\\\\/\\\\/+/, '/').replace(/^\\\\/+/, '')));"
            )
            .into_bytes(),
        ),
        (
            b"return \"/\".concat(path.trimLeft('/'));".to_vec(),
            b"return path.startsWith('/') ? path : \"/\".concat(path);".to_vec(),
        ),
        (
            b"return \\\"/\\\".concat(path.trimLeft('/'));".to_vec(),
            b"return path.startsWith('/') ? path : \\\"/\\\".concat(path);".to_vec(),
        ),
        (
            b"if (path.startsWith('http')) {".to_vec(),
            b"if (path.toLowerCase().startsWith('http://') || path.toLowerCase().startsWith('https://')) {"
                .to_vec(),
        ),
    ];
    let member_expression_rules = [
        (
            b"basename: \"\".concat(".to_vec(),
            b", \"/consolev3\")".to_vec(),
            format!("basename: \"{base_path}/\".concat(").into_bytes(),
        ),
        (
            b"basename:\"\".concat(".to_vec(),
            b",\"/consolev3\")".to_vec(),
            format!("basename:\"{base_path}/\".concat(").into_bytes(),
        ),
        (
            b"basename: \\\"\\\".concat(".to_vec(),
            b", \\\"/consolev3\\\")".to_vec(),
            format!("basename: \\\"{base_path}/\\\".concat(").into_bytes(),
        ),
        (
            b"basename:\\\"\\\".concat(".to_vec(),
            b",\\\"/consolev3\\\")".to_vec(),
            format!("basename:\\\"{base_path}/\\\".concat(").into_bytes(),
        ),
    ];

    StreamingRewritePipeline::new_with_exact_and_member_expression_patterns(
        [
            (source.to_vec(), replacement.to_vec()),
            (static_source, static_replacement),
        ],
        exact_rules,
        member_expression_rules,
        max_bytes,
    )
    .and_then(|pipeline| {
        pipeline.with_identifier_template_patterns([
            (
                b"\"/\".concat(".to_vec(),
                b".trimLeft(\"/\"))".to_vec(),
                format!(
                    "({{identifier}}===\"{base_path}\"||{{identifier}}.startsWith(\"{base_path}/\")?{{identifier}}:\"{base_path}/\".concat({{identifier}}.replace(/^\\/+/,\"\")))"
                )
                .into_bytes(),
            ),
            (
                b"return\"/\".concat(".to_vec(),
                b".trimLeft(\"/\"))".to_vec(),
                format!(
                    "return({{identifier}}===\"{base_path}\"||{{identifier}}.startsWith(\"{base_path}/\")?{{identifier}}:\"{base_path}/\".concat({{identifier}}.replace(/^\\/+/,\"\")))"
                )
                .into_bytes(),
            ),
            (
                b")),".to_vec(),
                b".replace(/\\/\\/+/,\"/\")}".to_vec(),
                format!(
                    ")),({{identifier}}={{identifier}}.replace(/\\/\\/+/,\"/\"),{{identifier}}===\"{base_path}\"||{{identifier}}.startsWith(\"{base_path}/\")?{{identifier}}:\"{base_path}/\".concat({{identifier}}.replace(/^\\/+/,\"\")))}}"
                )
                .into_bytes(),
            ),
        ])
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &[u8] = b"/extensions-static/ks-console-embed/dist/v3dist/";
    const REPLACEMENT: &[u8] =
        b"/regions/region:shenzhen/extensions-static/ks-console-embed/dist/v3dist/";

    #[test]
    fn response_rewriter_handles_router_basename_variants_idempotently() {
        let long_identifier = format!("a{}", "b".repeat(128));
        let input = format!(
            "spaced=basename: \"\".concat(webPrefix, \"/consolev3\");compact=basename:\"\".concat(o,\"/consolev3\");member=basename:\"\".concat(r.webPrefix,\"/consolev3\");escaped-spaced=basename: \\\"\\\".concat($router_2, \\\"/consolev3\\\");escaped-compact=basename:\\\"\\\".concat({long_identifier},\\\"/consolev3\\\");unrelated=basename:\"\".concat(apiPrefix,\"/other\");mybasename:\"\".concat(o,\"/consolev3\")"
        );
        let expected = format!(
            "spaced=basename: \"/regions/region:shenzhen/\".concat(webPrefix, \"/consolev3\");compact=basename:\"/regions/region:shenzhen/\".concat(o,\"/consolev3\");member=basename:\"/regions/region:shenzhen/\".concat(r.webPrefix,\"/consolev3\");escaped-spaced=basename: \\\"/regions/region:shenzhen/\\\".concat($router_2, \\\"/consolev3\\\");escaped-compact=basename:\\\"/regions/region:shenzhen/\\\".concat({long_identifier},\\\"/consolev3\\\");unrelated=basename:\"\".concat(apiPrefix,\"/other\");mybasename:\"\".concat(o,\"/consolev3\")"
        );

        for split in 0..=input.len() {
            let mut pipeline =
                build_response_rewriter("/regions/region:shenzhen", SOURCE, REPLACEMENT, 1024)
                    .expect("valid rewrite rules");
            let mut output = pipeline
                .push(&input.as_bytes()[..split])
                .expect("first chunk");
            output.extend(
                pipeline
                    .push(&input.as_bytes()[split..])
                    .expect("second chunk"),
            );
            output.extend(pipeline.finish().expect("finish stream"));
            assert_eq!(output, expected.as_bytes(), "split at byte {split}");

            let mut second_pass =
                build_response_rewriter("/regions/region:shenzhen", SOURCE, REPLACEMENT, 1024)
                    .expect("valid rewrite rules");
            let mut idempotent_output = second_pass.push(&output).expect("second pass");
            idempotent_output.extend(second_pass.finish().expect("finish second pass"));
            assert_eq!(
                idempotent_output,
                expected.as_bytes(),
                "second pass after byte {split}"
            );
        }
    }

    #[test]
    fn response_rewriter_prefixes_minified_request_url_normalizers() {
        let input = concat!(
            r#"function H(e){return e.startsWith("http")?e:"/".concat(e.trimLeft("/"))}"#,
            r#"function g(r){return function(e){if(e.startsWith("http"))return e;"#,
            r#"return"/".concat(e.trimLeft("/"))}(r)}"#
        );
        let expected = concat!(
            r#"function H(e){return e.startsWith("http")?e:(e==="/regions/region:shenzhen"||e.startsWith("/regions/region:shenzhen/")?e:"/regions/region:shenzhen/".concat(e.replace(/^\/+/,"")))}"#,
            r#"function g(r){return function(e){if(e.startsWith("http"))return e;"#,
            r#"return(e==="/regions/region:shenzhen"||e.startsWith("/regions/region:shenzhen/")?e:"/regions/region:shenzhen/".concat(e.replace(/^\/+/,"")))}(r)}"#
        );

        for split in 0..=input.len() {
            let mut pipeline =
                build_response_rewriter("/regions/region:shenzhen", SOURCE, REPLACEMENT, 1024)
                    .expect("valid rewrite rules");
            let mut output = pipeline
                .push(&input.as_bytes()[..split])
                .expect("first chunk");
            output.extend(
                pipeline
                    .push(&input.as_bytes()[split..])
                    .expect("second chunk"),
            );
            output.extend(pipeline.finish().expect("finish stream"));
            assert_eq!(output, expected.as_bytes(), "split at byte {split}");
        }
    }

    #[test]
    fn response_rewriter_preserves_base_path_after_minified_cluster_url_normalization() {
        let input = concat!(
            r#"function f(e){var t=e,a=t.match(o);return "#,
            r#"a&&(t="/".concat(a[2])),t.replace(/\/\/+/,"/")}"#
        );
        let expected = concat!(
            r#"function f(e){var t=e,a=t.match(o);return "#,
            r#"a&&(t="/".concat(a[2])),(t=t.replace(/\/\/+/,"/"),"#,
            r#"t==="/regions/region:shenzhen"||t.startsWith("/regions/region:shenzhen/")?"#,
            r#"t:"/regions/region:shenzhen/".concat(t.replace(/^\/+/,"")))}"#
        );

        for split in 0..=input.len() {
            let mut pipeline =
                build_response_rewriter("/regions/region:shenzhen", SOURCE, REPLACEMENT, 1024)
                    .expect("valid rewrite rules");
            let mut output = pipeline
                .push(&input.as_bytes()[..split])
                .expect("first chunk");
            output.extend(
                pipeline
                    .push(&input.as_bytes()[split..])
                    .expect("second chunk"),
            );
            output.extend(pipeline.finish().expect("finish stream"));
            assert_eq!(output, expected.as_bytes(), "split at byte {split}");
        }
    }

    #[test]
    fn jsbundle_rewriter_prefixes_window_host_urls_idempotently() {
        let input = concat!(
            r#"const bundleName="observability",ot=`${bundleName}-console-v3`,"#,
            r#"ut=`//${window.location.host}/${bundleName}/consolev3`,"#,
            r#"{V3ModalObserver:ct}=getEmbed({name:ot,baseUrl:ut});"#,
            r#"const another=`//${window.location.host}/${other_2}/consolev3`;"#,
            r#"const fixed=`//${window.location.host}/whizard-telemetry/consolev3`;"#
        );
        let expected = concat!(
            r#"const bundleName="observability",ot=`${bundleName}-console-v3`,"#,
            r#"ut=`//${window.location.host}/regions/region:shenzhen/${bundleName}/consolev3`,"#,
            r#"{V3ModalObserver:ct}=getEmbed({name:ot,baseUrl:ut});"#,
            r#"const another=`//${window.location.host}/regions/region:shenzhen/${other_2}/consolev3`;"#,
            r#"const fixed=`//${window.location.host}/regions/region:shenzhen/whizard-telemetry/consolev3`;"#
        );

        for split in 0..=input.len() {
            let mut pipeline = build_selected_response_rewriter(
                RewriteProfile::JsBundle,
                "/regions/region:shenzhen",
                "observability",
                1024,
            )
            .expect("valid rewrite rule");
            let mut output = pipeline
                .push(&input.as_bytes()[..split])
                .expect("first chunk");
            output.extend(
                pipeline
                    .push(&input.as_bytes()[split..])
                    .expect("second chunk"),
            );
            output.extend(pipeline.finish().expect("finish stream"));
            assert_eq!(output, expected.as_bytes(), "split at byte {split}");
        }

        for split in 0..=expected.len() {
            let mut second_pass = build_selected_response_rewriter(
                RewriteProfile::JsBundle,
                "/regions/region:shenzhen",
                "observability",
                1024,
            )
            .expect("valid rewrite rule");
            let mut idempotent_output = second_pass
                .push(&expected.as_bytes()[..split])
                .expect("first idempotent chunk");
            idempotent_output.extend(
                second_pass
                    .push(&expected.as_bytes()[split..])
                    .expect("second idempotent chunk"),
            );
            idempotent_output.extend(second_pass.finish().expect("finish second pass"));
            assert_eq!(
                idempotent_output,
                expected.as_bytes(),
                "second pass split at byte {split}"
            );
        }
    }

    #[test]
    fn kubekey_rewriter_prefixes_asset_urls_across_chunk_boundaries_idempotently() {
        let input = r#"<!doctype html><link rel="icon" href="/proxy/kubekey/favicon.svg"><script src="/proxy/kubekey/assets/index.js"></script><link href="/proxy/kubekey/assets/index.css"><a href="/other">other</a>"#;
        let expected = r#"<!doctype html><link rel="icon" href="/regions/region:region-04/proxy/kubekey/favicon.svg"><script src="/regions/region:region-04/proxy/kubekey/assets/index.js"></script><link href="/regions/region:region-04/proxy/kubekey/assets/index.css"><a href="/other">other</a>"#;

        for split in 0..=input.len() {
            let mut pipeline = build_selected_response_rewriter(
                RewriteProfile::NamedProxyHtml,
                "/regions/region:region-04",
                "kubekey",
                1024,
            )
            .expect("valid rewrite rule");
            let mut output = pipeline
                .push(&input.as_bytes()[..split])
                .expect("first chunk");
            output.extend(
                pipeline
                    .push(&input.as_bytes()[split..])
                    .expect("second chunk"),
            );
            output.extend(pipeline.finish().expect("finish stream"));
            assert_eq!(output, expected.as_bytes(), "split at byte {split}");

            let mut second_pass = build_selected_response_rewriter(
                RewriteProfile::NamedProxyHtml,
                "/regions/region:region-04",
                "kubekey",
                1024,
            )
            .expect("valid rewrite rule");
            let mut idempotent_output = second_pass.push(&output).expect("second pass");
            idempotent_output.extend(second_pass.finish().expect("finish second pass"));
            assert_eq!(
                idempotent_output,
                expected.as_bytes(),
                "second pass after byte {split}"
            );
        }
    }

    #[test]
    fn named_proxy_js_rewriter_prefixes_its_own_proxy_root_idempotently() {
        let input = r#"const root="/proxy/kubekey";const asset="/proxy/kubekey/assets/data.json";const other="/proxy/another-app";"#;
        let expected = r#"const root="/regions/region:region-04/proxy/kubekey";const asset="/regions/region:region-04/proxy/kubekey/assets/data.json";const other="/proxy/another-app";"#;

        for split in 0..=input.len() {
            let mut pipeline = build_selected_response_rewriter(
                RewriteProfile::ProxyJs,
                "/regions/region:region-04",
                "kubekey",
                1024,
            )
            .expect("valid rewrite rule");
            let mut output = pipeline
                .push(&input.as_bytes()[..split])
                .expect("first chunk");
            output.extend(
                pipeline
                    .push(&input.as_bytes()[split..])
                    .expect("second chunk"),
            );
            output.extend(pipeline.finish().expect("finish stream"));
            assert_eq!(output, expected.as_bytes(), "split at byte {split}");

            let mut second_pass = build_selected_response_rewriter(
                RewriteProfile::ProxyJs,
                "/regions/region:region-04",
                "kubekey",
                1024,
            )
            .expect("valid rewrite rule");
            let mut idempotent_output = second_pass.push(&output).expect("second pass");
            idempotent_output.extend(second_pass.finish().expect("finish second pass"));
            assert_eq!(
                idempotent_output,
                expected.as_bytes(),
                "second pass after byte {split}"
            );
        }
    }

    #[test]
    fn kubekey_asset_js_rewriter_prefixes_proxy_and_kapis_roots_idempotently() {
        let input = r#"const ui="/proxy/kubekey/assets/data.json";const api="/kapis/kubekey.kubesphere.io/v1alpha1/install";const other="/kapis/another.kubesphere.io";"#;
        let expected = r#"const ui="/regions/region:region-04/proxy/kubekey/assets/data.json";const api="/regions/region:region-04/kapis/kubekey.kubesphere.io/v1alpha1/install";const other="/kapis/another.kubesphere.io";"#;

        for split in 0..=input.len() {
            let mut pipeline = build_selected_response_rewriter(
                RewriteProfile::KubekeyAssetJs,
                "/regions/region:region-04",
                "kubekey",
                1024,
            )
            .expect("valid rewrite rules");
            let mut output = pipeline
                .push(&input.as_bytes()[..split])
                .expect("first chunk");
            output.extend(
                pipeline
                    .push(&input.as_bytes()[split..])
                    .expect("second chunk"),
            );
            output.extend(pipeline.finish().expect("finish stream"));
            assert_eq!(output, expected.as_bytes(), "split at byte {split}");

            let mut second_pass = build_selected_response_rewriter(
                RewriteProfile::KubekeyAssetJs,
                "/regions/region:region-04",
                "kubekey",
                1024,
            )
            .expect("valid rewrite rules");
            let mut idempotent_output = second_pass.push(&output).expect("second pass");
            idempotent_output.extend(second_pass.finish().expect("finish second pass"));
            assert_eq!(
                idempotent_output,
                expected.as_bytes(),
                "second pass after byte {split}"
            );
        }
    }

    #[test]
    fn ys1000_index_html_rewriter_prefixes_resource_urls_idempotently() {
        let input = "<!DOCTYPE html><link rel=\"icon\" href=\"/proxy/ys1000/favicon.ico\"><link rel=\"stylesheet\"\n\thref='/proxy/ys1000/main.css'><script src=\"/proxy/ys1000/app.bundle.js\"></script><script\x0Csrc='/proxy/ys1000/form-feed.js'></script><img data-src=\"/proxy/ys1000/lazy.png\"><svg xlink:href=\"/proxy/ys1000/icon.svg\"></svg><script src='https://cdn.example/proxy/ys1000/external.js'></script><a href=\"/proxy/ys1000-old/\">old</a><a href=\"/proxy/another-app/\">other</a>";
        let expected = "<!DOCTYPE html><link rel=\"icon\" href=\"/regions/region:region-04/proxy/ys1000/favicon.ico\"><link rel=\"stylesheet\"\n\thref='/regions/region:region-04/proxy/ys1000/main.css'><script src=\"/regions/region:region-04/proxy/ys1000/app.bundle.js\"></script><script\x0Csrc='/regions/region:region-04/proxy/ys1000/form-feed.js'></script><img data-src=\"/proxy/ys1000/lazy.png\"><svg xlink:href=\"/proxy/ys1000/icon.svg\"></svg><script src='https://cdn.example/proxy/ys1000/external.js'></script><a href=\"/proxy/ys1000-old/\">old</a><a href=\"/proxy/another-app/\">other</a>";

        for split in 0..=input.len() {
            let mut pipeline = build_selected_response_rewriter(
                RewriteProfile::NamedProxyHtml,
                "/regions/region:region-04",
                "ys1000",
                1024,
            )
            .expect("valid rewrite rule");
            let mut output = pipeline
                .push(&input.as_bytes()[..split])
                .expect("first chunk");
            output.extend(
                pipeline
                    .push(&input.as_bytes()[split..])
                    .expect("second chunk"),
            );
            output.extend(pipeline.finish().expect("finish stream"));
            assert_eq!(output, expected.as_bytes(), "split at byte {split}");

            let mut second_pass = build_selected_response_rewriter(
                RewriteProfile::NamedProxyHtml,
                "/regions/region:region-04",
                "ys1000",
                1024,
            )
            .expect("valid rewrite rule");
            let mut idempotent_output = second_pass.push(&output).expect("second pass");
            idempotent_output.extend(second_pass.finish().expect("finish second pass"));
            assert_eq!(
                idempotent_output,
                expected.as_bytes(),
                "second pass after byte {split}"
            );
        }
    }
}
