use std::collections::HashSet;

use crate::literal::{RewriteError, StreamingRewritePipeline};

pub(crate) const REWRITE_RULE_VERSION: &str = "v19";
pub(crate) const ALL_EXTENSIONS_WILDCARD: &str = "*";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RewriteProfile {
    ConsoleV3,
    JsBundle,
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
        Self::from_matcher(base_path, extensions)
    }

    pub fn for_all_extensions(base_path: impl Into<String>) -> Self {
        Self::from_matcher(base_path, ExtensionMatcher::All)
    }

    pub fn for_allowlisted_extensions<I, S>(
        base_path: impl Into<String>,
        enabled_extensions: I,
    ) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let enabled_extensions = enabled_extensions
            .into_iter()
            .map(|extension| extension.as_ref().to_string())
            .collect();
        Self::from_matcher(base_path, ExtensionMatcher::Allowlist(enabled_extensions))
    }

    pub fn decide(&self, method: &str, path: &str) -> RewriteDecision {
        if self.base_path.is_empty()
            || (!method.eq_ignore_ascii_case("GET") && !method.eq_ignore_ascii_case("HEAD"))
        {
            return RewriteDecision::Bypass;
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
                profile: RewriteProfile::JsBundle,
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

    pub fn metrics_extension_label<'a>(&self, extension: &'a str) -> &'a str {
        match self.extensions {
            ExtensionMatcher::All => ALL_EXTENSIONS_WILDCARD,
            ExtensionMatcher::Allowlist(_) => extension,
        }
    }

    fn is_extension_enabled(&self, extension: &str) -> bool {
        is_safe_extension_name(extension)
            && match &self.extensions {
                ExtensionMatcher::All => true,
                ExtensionMatcher::Allowlist(enabled_extensions) => {
                    enabled_extensions.contains(extension)
                }
            }
    }

    fn from_matcher(base_path: impl Into<String>, extensions: ExtensionMatcher) -> Self {
        Self {
            base_path: base_path.into().trim_end_matches('/').to_string(),
            extensions,
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
        RewriteProfile::JsBundle => StreamingRewritePipeline::new_with_appended_suffix(
            b"`//${window.location.host}/",
            format!("{}/", base_path.trim_start_matches('/')),
            max_bytes,
        ),
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
}
