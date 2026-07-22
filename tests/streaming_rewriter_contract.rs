use kse_static_rewrite_proxy::literal::StreamingLiteralRewriter;

const SOURCE: &[u8] = b"/extensions-static/ks-console-embed/dist/v3dist/";
const REPLACEMENT: &[u8] =
    b"/regions/region:shenzhen/extensions-static/ks-console-embed/dist/v3dist/";

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
