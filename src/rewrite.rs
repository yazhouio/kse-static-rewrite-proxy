use std::collections::HashSet;

use crate::literal::{RewriteError, StreamingRewritePipeline};

pub(crate) const REWRITE_RULE_VERSION: &str = "v17";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RewriteProfile {
    ConsoleV3,
    KubeEyeJsBundle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RewriteDecision {
    Bypass,
    Rewrite {
        profile: RewriteProfile,
        extension: String,
        source: Vec<u8>,
        replacement: Vec<u8>,
        head_only: bool,
    },
}

#[derive(Debug, Clone)]
pub struct RewritePolicy {
    base_path: String,
    enabled_extensions: HashSet<String>,
}

impl RewritePolicy {
    pub fn new<I, S>(base_path: impl Into<String>, enabled_extensions: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self {
            base_path: base_path.into().trim_end_matches('/').to_string(),
            enabled_extensions: enabled_extensions
                .into_iter()
                .map(|extension| extension.as_ref().to_string())
                .collect(),
        }
    }

    pub fn decide(&self, method: &str, path: &str) -> RewriteDecision {
        if self.base_path.is_empty()
            || (!method.eq_ignore_ascii_case("GET") && !method.eq_ignore_ascii_case("HEAD"))
        {
            return RewriteDecision::Bypass;
        }

        let kubeeye_prefix = format!("{}/jsbundles/kubeeye/dist/kubeeye/", self.base_path);
        if self.enabled_extensions.contains("kubeeye") {
            if let Some(asset_path) = path.strip_prefix(&kubeeye_prefix) {
                if !asset_path.is_empty()
                    && !asset_path.contains('/')
                    && asset_path.ends_with(".js")
                {
                    return RewriteDecision::Rewrite {
                        profile: RewriteProfile::KubeEyeJsBundle,
                        extension: "kubeeye".to_owned(),
                        source: b"`//${window.location.host}/${rt}/consolev3`".to_vec(),
                        replacement: format!(
                            "`//${{window.location.host}}{}/${{rt}}/consolev3`",
                            self.base_path
                        )
                        .into_bytes(),
                        head_only: method.eq_ignore_ascii_case("HEAD"),
                    };
                }
            }
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
            || !self.enabled_extensions.contains(extension)
            || !is_text_asset(asset_path)
        {
            return RewriteDecision::Bypass;
        }

        let source = format!("/extensions-static/{extension}/dist/v3dist/");
        let replacement = format!("{}{}", self.base_path, source);
        RewriteDecision::Rewrite {
            profile: RewriteProfile::ConsoleV3,
            extension: extension.to_string(),
            source: source.into_bytes(),
            replacement: replacement.into_bytes(),
            head_only: method.eq_ignore_ascii_case("HEAD"),
        }
    }
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
    source: &[u8],
    replacement: &[u8],
    max_bytes: usize,
) -> Result<StreamingRewritePipeline, RewriteError> {
    match profile {
        RewriteProfile::ConsoleV3 => {
            build_response_rewriter(base_path, source, replacement, max_bytes)
        }
        RewriteProfile::KubeEyeJsBundle => StreamingRewritePipeline::new_with_exact(
            std::iter::empty::<(Vec<u8>, Vec<u8>)>(),
            [(source.to_vec(), replacement.to_vec())],
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
    let identifier_rules = [
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

    StreamingRewritePipeline::new_with_exact_and_identifier_patterns(
        [
            (source.to_vec(), replacement.to_vec()),
            (static_source, static_replacement),
        ],
        exact_rules,
        identifier_rules,
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
            "spaced=basename: \"\".concat(webPrefix, \"/consolev3\");compact=basename:\"\".concat(o,\"/consolev3\");escaped-spaced=basename: \\\"\\\".concat($router_2, \\\"/consolev3\\\");escaped-compact=basename:\\\"\\\".concat({long_identifier},\\\"/consolev3\\\");unrelated=basename:\"\".concat(apiPrefix,\"/other\");mybasename:\"\".concat(o,\"/consolev3\")"
        );
        let expected = format!(
            "spaced=basename: \"/regions/region:shenzhen/\".concat(webPrefix, \"/consolev3\");compact=basename:\"/regions/region:shenzhen/\".concat(o,\"/consolev3\");escaped-spaced=basename: \\\"/regions/region:shenzhen/\\\".concat($router_2, \\\"/consolev3\\\");escaped-compact=basename:\\\"/regions/region:shenzhen/\\\".concat({long_identifier},\\\"/consolev3\\\");unrelated=basename:\"\".concat(apiPrefix,\"/other\");mybasename:\"\".concat(o,\"/consolev3\")"
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
    fn kubeeye_rewriter_prefixes_only_the_console_v3_base_url_idempotently() {
        let source = b"`//${window.location.host}/${rt}/consolev3`";
        let replacement = b"`//${window.location.host}/regions/region:shenzhen/${rt}/consolev3`";
        let input = concat!(
            r#"const rt="kubeeye",ot=`${rt}-console-v3`,"#,
            r#"ut=`//${window.location.host}/${rt}/consolev3`,"#,
            r#"{V3ModalObserver:ct}=getEmbed({name:ot,baseUrl:ut});"#,
            r#"const untouched=`//${window.location.host}/${other}/consolev3`;"#
        );
        let expected = concat!(
            r#"const rt="kubeeye",ot=`${rt}-console-v3`,"#,
            r#"ut=`//${window.location.host}/regions/region:shenzhen/${rt}/consolev3`,"#,
            r#"{V3ModalObserver:ct}=getEmbed({name:ot,baseUrl:ut});"#,
            r#"const untouched=`//${window.location.host}/${other}/consolev3`;"#
        );

        for split in 0..=input.len() {
            let mut pipeline = build_selected_response_rewriter(
                RewriteProfile::KubeEyeJsBundle,
                "/regions/region:shenzhen",
                source,
                replacement,
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
                RewriteProfile::KubeEyeJsBundle,
                "/regions/region:shenzhen",
                source,
                replacement,
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
