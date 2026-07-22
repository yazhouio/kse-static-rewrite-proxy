use std::collections::HashSet;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RewriteDecision {
    Bypass,
    Rewrite {
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
