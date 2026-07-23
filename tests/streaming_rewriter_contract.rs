use kse_static_rewrite_proxy::literal::{
    RewriteError, StreamingLiteralRewriter, StreamingRewritePipeline,
};

const SOURCE: &[u8] = b"/extensions-static/ks-console-embed/dist/v3dist/";
const REPLACEMENT: &[u8] =
    b"/regions/region:shenzhen/extensions-static/ks-console-embed/dist/v3dist/";
const STATIC_SOURCE: &[u8] = b"/extensions-static/";
const STATIC_REPLACEMENT: &[u8] = b"/regions/region:shenzhen/extensions-static/";
const ROUTER_SOURCE: &[u8] = b"basename: \"\".concat(webPrefix, \"/consolev3\")";
const ROUTER_REPLACEMENT: &[u8] =
    b"basename: \"/regions/region:shenzhen/\".concat(webPrefix, \"/consolev3\")";
const ESCAPED_ROUTER_SOURCE: &[u8] = b"basename: \\\"\\\".concat(webPrefix, \\\"/consolev3\\\")";
const ESCAPED_ROUTER_REPLACEMENT: &[u8] =
    b"basename: \\\"/regions/region:shenzhen/\\\".concat(webPrefix, \\\"/consolev3\\\")";
const API_SOURCE: &[u8] = b"return requestURL.replace(/\\\\/\\\\/+/, '/');";
const API_REPLACEMENT: &[u8] = b"return requestURL.toLowerCase().startsWith('http://') || requestURL.toLowerCase().startsWith('https://') || requestURL.startsWith('//') ? requestURL : (requestURL.replace(/\\\\/\\\\/+/, '/') === '/regions/region:shenzhen' || requestURL.replace(/\\\\/\\\\/+/, '/').startsWith('/regions/region:shenzhen/') ? requestURL.replace(/\\\\/\\\\/+/, '/') : '/regions/region:shenzhen/'.concat(requestURL.replace(/\\\\/\\\\/+/, '/').replace(/^\\\\/+/, '')));";
const CREATE_URL_SOURCE: &[u8] = b"return \"/\".concat(path.trimLeft('/'));";
const CREATE_URL_REPLACEMENT: &[u8] = b"return path.startsWith('/') ? path : \"/\".concat(path);";
const ESCAPED_CREATE_URL_SOURCE: &[u8] = b"return \\\"/\\\".concat(path.trimLeft('/'));";
const ESCAPED_CREATE_URL_REPLACEMENT: &[u8] =
    b"return path.startsWith('/') ? path : \\\"/\\\".concat(path);";
const CREATE_URL_HTTP_SOURCE: &[u8] = b"if (path.startsWith('http')) {";
const CREATE_URL_HTTP_REPLACEMENT: &[u8] =
    b"if (path.toLowerCase().startsWith('http://') || path.toLowerCase().startsWith('https://')) {";

#[test]
fn replaces_across_every_chunk_boundary_without_double_prefixing() {
    let input = b"before:/extensions-static/ks-console-embed/dist/v3dist/locale-zh.json;already:/regions/region:shenzhen/extensions-static/ks-console-embed/dist/v3dist/main.js;after";
    let expected = b"before:/regions/region:shenzhen/extensions-static/ks-console-embed/dist/v3dist/locale-zh.json;already:/regions/region:shenzhen/extensions-static/ks-console-embed/dist/v3dist/main.js;after";

    for split in 0..=input.len() {
        let mut rewriter =
            StreamingLiteralRewriter::new(SOURCE, REPLACEMENT, 1024).expect("valid rewrite rule");
        let mut output = rewriter.push(&input[..split]).expect("first chunk");
        output.extend(rewriter.push(&input[split..]).expect("second chunk"));
        output.extend(rewriter.finish().expect("finish stream"));
        assert_eq!(output, expected, "split at byte {split}");
    }
}

#[test]
fn validates_utf8_sequences_split_across_every_byte_boundary() {
    let input = "前缀:/extensions-static/文件.js".as_bytes();
    let expected = "前缀:/regions/region:shenzhen/extensions-static/文件.js".as_bytes();

    for chunk_size in 1..=input.len() {
        let mut rewriter =
            StreamingLiteralRewriter::new(STATIC_SOURCE, STATIC_REPLACEMENT, 1024).unwrap();
        let mut output = Vec::new();
        for chunk in input.chunks(chunk_size) {
            output.extend(rewriter.push(chunk).expect("valid UTF-8 chunk"));
        }
        output.extend(rewriter.finish().expect("complete UTF-8 stream"));
        assert_eq!(output, expected, "chunk size {chunk_size}");
    }
}

#[test]
fn rejects_invalid_utf8_after_an_incomplete_sequence() {
    let mut rewriter =
        StreamingLiteralRewriter::new(STATIC_SOURCE, STATIC_REPLACEMENT, 1024).unwrap();

    assert_eq!(rewriter.push(&[0xE4]), Ok(Vec::new()));
    assert_eq!(rewriter.push(b"A"), Err(RewriteError::InvalidUtf8));
    assert_eq!(rewriter.push(&[0xB8, 0xAD]), Err(RewriteError::InvalidUtf8));
}

#[test]
fn rewrites_dense_adjacent_literals_without_reprocessing_output() {
    let mut prefix_rewriter = StreamingLiteralRewriter::new(b"x", b"px", 1024).unwrap();
    let mut prefix_output = prefix_rewriter.push(b"xxxx").unwrap();
    prefix_output.extend(prefix_rewriter.finish().unwrap());
    assert_eq!(prefix_output, b"pxpxpxpx");

    let mut exact_rewriter = StreamingLiteralRewriter::new_exact(b"ab", b"z", 1024).unwrap();
    let mut exact_output = exact_rewriter.push(b"ababab").unwrap();
    exact_output.extend(exact_rewriter.finish().unwrap());
    assert_eq!(exact_output, b"zzz");
}

#[test]
fn rewrites_runtime_composed_static_roots_across_every_chunk_boundary() {
    let input = b"request.get(\"\".concat(webPrefix ? \"/extensions-static/\".concat(webPrefix) : \"\", \"/dist/v3dist/\").concat(localePath));basename: \"\".concat(webPrefix, \"/consolev3\");eval=basename: \\\"\\\".concat(webPrefix, \\\"/consolev3\\\");return requestURL.replace(/\\\\/\\\\/+/, '/');if (path.startsWith('http')) {return \"/\".concat(path.trimLeft('/'));eval=return \\\"/\\\".concat(path.trimLeft('/'));kept=\"/regions/region:shenzhen/extensions-static/ks-console-embed/dist/v3dist/main.js\"";
    let expected = b"request.get(\"\".concat(webPrefix ? \"/regions/region:shenzhen/extensions-static/\".concat(webPrefix) : \"\", \"/dist/v3dist/\").concat(localePath));basename: \"/regions/region:shenzhen/\".concat(webPrefix, \"/consolev3\");eval=basename: \\\"/regions/region:shenzhen/\\\".concat(webPrefix, \\\"/consolev3\\\");return requestURL.toLowerCase().startsWith('http://') || requestURL.toLowerCase().startsWith('https://') || requestURL.startsWith('//') ? requestURL : (requestURL.replace(/\\\\/\\\\/+/, '/') === '/regions/region:shenzhen' || requestURL.replace(/\\\\/\\\\/+/, '/').startsWith('/regions/region:shenzhen/') ? requestURL.replace(/\\\\/\\\\/+/, '/') : '/regions/region:shenzhen/'.concat(requestURL.replace(/\\\\/\\\\/+/, '/').replace(/^\\\\/+/, '')));if (path.toLowerCase().startsWith('http://') || path.toLowerCase().startsWith('https://')) {return path.startsWith('/') ? path : \"/\".concat(path);eval=return path.startsWith('/') ? path : \\\"/\\\".concat(path);kept=\"/regions/region:shenzhen/extensions-static/ks-console-embed/dist/v3dist/main.js\"";

    for split in 0..=input.len() {
        let mut pipeline = StreamingRewritePipeline::new_with_exact(
            [(SOURCE, REPLACEMENT), (STATIC_SOURCE, STATIC_REPLACEMENT)],
            [
                (ROUTER_SOURCE, ROUTER_REPLACEMENT),
                (ESCAPED_ROUTER_SOURCE, ESCAPED_ROUTER_REPLACEMENT),
                (API_SOURCE, API_REPLACEMENT),
                (CREATE_URL_SOURCE, CREATE_URL_REPLACEMENT),
                (ESCAPED_CREATE_URL_SOURCE, ESCAPED_CREATE_URL_REPLACEMENT),
                (CREATE_URL_HTTP_SOURCE, CREATE_URL_HTTP_REPLACEMENT),
            ],
            1024,
        )
        .expect("valid rewrite rules");
        let mut output = pipeline.push(&input[..split]).expect("first chunk");
        output.extend(pipeline.push(&input[split..]).expect("second chunk"));
        output.extend(pipeline.finish().expect("finish stream"));
        assert_eq!(output, expected, "split at byte {split}");
    }
}
