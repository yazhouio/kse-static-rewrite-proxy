# KSE static rewrite proxy

[English](README.md) | [简体中文](README.zh-CN.md)

A temporary, independently deployable Pingora sidecar for KSE Console. It forwards every Console request to the BFF unchanged, except for narrowly scoped stream rewrites in configured extension assets.

## Request flow

```text
Gateway / Ingress
       |
       v
Pingora sidecar :8080 ----> KSE Console BFF 127.0.0.1:8000
       |
       +--- admin :9090 (Pod probes / Prometheus only)
```

The Console Service must target sidecar port `8080`; the BFF remains an internal same-Pod upstream. The sidecar preserves the request path, host, cookies, login/logout behavior, APIs, WebSockets, SSE, and uploads.

Health and metrics use a separate admin listener on `9090`. The Console Service exposes only `8080`, so wildcard Console Ingress routes cannot reach or shadow the admin endpoints.

## Rewrite scope

### Rewrite rule summary (v33)

There are **3 top-level request path roots and 6 response rewrite rules**. The
request roots are `extensions-static`, `jsbundles`, and `proxy`. Response rules
correspond to the `RewriteProfile` variants in the code. Multiple literal forms
handled within one rule for build-output compatibility are not counted
separately.

| # | Rule | Request path | Response requirements | Rewrite |
|---|---|---|---|---|
| 1 | Console V3 static assets | `{basePath}/extensions-static/{extension}/dist/v3dist/**` | Enabled and not disabled extension; filename ends in `.js`, `.mjs`, `.css`, `.json`, `.html`, or `.htm`; supported UTF-8 text type | Prefix extension static roots with `basePath`, including standalone `/extensions-static/`, React Router `basename`, and relative API URL normalization compatibility |
| 2 | JSBundle | `{basePath}/jsbundles/{extension}/dist/{distribution}/*.js` | Enabled and not disabled extension; `distribution` equals `extension`, or equals `{name}` when the outer extension is `{name}-frontend`; only direct `.js` files | Replace `` `//${window.location.host}/ `` with `` `//${window.location.host}{basePath}/ `` |
| 3 | Frontend Index JSBundle | `{basePath}/jsbundles/{name}-frontend/dist/{name}-frontend/index.js` or `.../dist/{name}/index.js` | Enabled and not disabled extension; additionally accepts `text/plain` with no charset or UTF-8/UTF8 charset | Same content rewrite as JSBundle; separate rule for Content-Type compatibility |
| 4 | Named Proxy HTML | `{basePath}/proxy/{name}/` and any descendant path | Only `text/html` or `application/xhtml+xml`; other types bypass unchanged | Prefix lowercase, tightly formatted `href="/proxy/{name}/..."` and `src="/proxy/{name}/..."` attributes with `basePath`; Kubekey HTML also replaces the fixed legacy root `/57516e69-2cb0-4d48-a8a8-2833cfff87a9` with `basePath` |
| 5 | Ys1000 MIG Meta HTML | `{basePath}/proxy/ys1000/` and any descendant path | Only `text/html` or `application/xhtml+xml`; other types bypass unchanged | Base64-decode the JSON in `window._mig_meta`, change top-level `baseURI` from `/proxy/ys1000` to `{basePath}/proxy/ys1000`, re-encode it in place, and include rule 4's HTML attribute rewrites |
| 6 | Kubekey Assets JavaScript | `{basePath}/proxy/kubekey/assets/**/*.js` | Supported UTF-8 text type; not controlled by the extension allowlist | Replace only the fixed legacy root `/57516e69-2cb0-4d48-a8a8-2833cfff87a9` with `basePath` |

Common behavior:

- Only `GET` and `HEAD` requests participate in matching. An empty `basePath`
  bypasses every rule.
- Only Console V3, JSBundle, and Frontend Index JSBundle are controlled by
  `enabledExtensions` / `disabledExtensions`. Named Proxy rules are not.
- Non-`200` upstream responses are unchanged. Selected responses must use
  identity encoding. Body rewrites support arbitrary HTTP chunk boundaries and
  are idempotent.
- A candidate selected by a Named Proxy HTML rule bypasses without a `502` if
  its final Content-Type is not HTML/XHTML, preserving range and conditional
  request semantics. Binary image, font, and video bodies remain unchanged,
  although candidate requests still ask the upstream for identity encoding.
- Ordinary Named Proxy JavaScript is no longer rewritten. Kubekey assets
  JavaScript is the only Named Proxy JavaScript exception and takes precedence
  over generic HTML.

### Complete rule details

#### 1. Console V3 static assets

The path must match
`{basePath}/extensions-static/{extension}/dist/v3dist/**`; the extension must be
enabled and not disabled; and the final filename must end in `.js`, `.mjs`,
`.css`, `.json`, `.html`, or `.htm`. Selected responses receive these
compatibility rewrites:

1. Extension-specific static root:

   ```text
   /extensions-static/{extension}/dist/v3dist/
           ->
   {basePath}/extensions-static/{extension}/dist/v3dist/
   ```

2. Standalone static root:

   ```text
   /extensions-static/
           ->
   {basePath}/extensions-static/
   ```

3. React Router `basename`: recognizes common minified, unminified, and
   escaped `basename: "".concat(..., "/consolev3")` forms and inserts
   `{basePath}/` before the `concat` argument.
4. API URL normalization: prefixes relative URLs with `basePath` and preserves
   URLs that already contain it.
5. Absolute URL protection: does not prefix `http://`, `https://`, or
   protocol-relative `//...` URLs.
6. Relative path concatenation compatibility: handles plain, escaped, return
   statement, and identifier-expression forms from build output without
   duplicating `/` or `basePath`.

These compatibility transforms apply only to the matched extension response;
they do not scan API responses or other extension assets.

#### 2. JSBundle

Only direct JavaScript files under
`{basePath}/jsbundles/{extension}/dist/{distribution}/*.js` match, and
`{extension}` must be enabled and not disabled. `distribution` may be:

- Exactly the same as the outer `{extension}`.
- `{name}` without `-frontend` when the outer extension is `{name}-frontend`.

For example, both paths match:

```text
.../jsbundles/ys1000-frontend/dist/ys1000-frontend/index.js
.../jsbundles/ks-autoscaling-frontend/dist/ks-autoscaling/index.js
```

Any other mismatched dist name and any deeper `.js` file do not match.

```text
`//${window.location.host}/
        ->
`//${window.location.host}{basePath}/
```

Fixed extension names and JavaScript interpolation forms both preserve the
remaining path. A template string that already contains the inserted content is
not prefixed again.

#### 3. Frontend Index JSBundle

Matches an enabled and not disabled `{name}-frontend` outer extension. Its dist
directory may be the full `{name}-frontend` or `{name}` without the suffix, and
the filename must be `index.js`. The content rewrite is the same as JSBundle.
It additionally accepts `text/plain` with no charset, or with a charset of
`utf-8` or `utf8`.

#### 4. Named Proxy HTML

Matches `{basePath}/proxy/{name}/` and any descendant path, but processes only
final responses of `text/html` or `application/xhtml+xml`. An HTML attribute
must satisfy every condition:

- The attribute name is lowercase `href` or `src`; names such as `data-src` and
  `xlink:href` do not match.
- The attribute is preceded by HTML ASCII whitespace: Space, Tab, CR, LF, or
  Form Feed.
- There is no whitespace around `=`, and the value uses single or double
  quotes.
- The value starts with `/proxy/{name}/`; other names, similar prefixes, and
  absolute external URLs do not match.

```text
href="/proxy/{name}/..."
src="/proxy/{name}/..."
        ->
href="{basePath}/proxy/{name}/..."
src="{basePath}/proxy/{name}/..."
```

When `{name}` is `kubekey`, the rule also performs this replacement anywhere in
the HTML body:

```text
/57516e69-2cb0-4d48-a8a8-2833cfff87a9
        ->
{basePath}
```

Subpaths after the fixed legacy root remain unchanged. Other Named Proxy HTML
does not perform this replacement.

If the final response is not HTML/XHTML, rewriting stops, the body is unchanged,
no `502` is returned, and range and conditional cache request semantics are
preserved. Because candidate requests already ask the upstream for identity
encoding, a bypassed response is not guaranteed to retain its original
compressed representation.

#### 5. Ys1000 MIG Meta HTML

Matches `{basePath}/proxy/ys1000/` and any descendant path and includes rule 4's
HTML attribute rewrites. For this assignment in the response body:

```text
window._mig_meta = '<Base64 JSON>';
```

The value is decoded using standard Base64. If the result is JSON and its
top-level `baseURI` is exactly `/proxy/ys1000`, it is changed to:

```text
{basePath}/proxy/ys1000
```

The modified JSON is standard-Base64 encoded and placed back in the original
assignment. Surrounding HTML, whitespace, and quote style remain unchanged.
Invalid Base64, invalid JSON, a missing `baseURI`, or any other `baseURI` value
is left unchanged. A value already equal to the target is not modified again.

#### 6. Kubekey Assets JavaScript

Matches `{basePath}/proxy/kubekey/assets/**/*.js` and performs only:

```text
/57516e69-2cb0-4d48-a8a8-2833cfff87a9
        ->
{basePath}
```

Subpaths after the fixed legacy root remain unchanged. Rule 6 does not modify
`"/proxy/kubekey"` or `/kapis/kubekey.kubesphere.io`. Every other Named Proxy
JavaScript response bypasses.

### Response types and cache behavior

- The standard text Content-Type allowlist is `text/javascript`,
  `application/javascript`, `application/x-javascript`, `text/css`,
  `application/json`, `text/json`, `text/html`, and
  `application/xhtml+xml`.
- A charset may be omitted. If present, it must be `utf-8` or `utf8`.
- Frontend Index JSBundle additionally accepts `text/plain` with those charset
  rules.
- Named Proxy HTML accepts only HTML/XHTML and bypasses a type mismatch. Other
  explicitly selected rules return `502` for unsupported Content-Types.
- The proxy requests identity encoding from the upstream. A selected response
  that still has a non-identity `Content-Encoding` returns `502`.
- A successful rewrite removes stale length, encoding, digest, range, and
  Last-Modified representation metadata. A reliable upstream ETag produces a
  new weak ETag that includes the current rule version; otherwise the response
  uses `Cache-Control: no-store`.
- Every body transform has a maximum decoded-byte limit, supports arbitrary
  HTTP chunk boundaries, and is idempotent when run again over already rewritten
  static output.
- A body larger than `maxDecodedBytes` or one that is not valid UTF-8 fails the
  rewrite with `502`.

## Configuration

The sidecar reads the same YAML document as the Console. Set `KSE_REWRITE_CONFIG` to change its location; the default is `/etc/kse-console/config.yaml`.

```yaml
client:
  basePath: /regions/region:shenzhen

rewriteSidecar:
  listen: 0.0.0.0:8080
  adminListen: 0.0.0.0:9090
  upstream: http://127.0.0.1:8000
  rewrite:
    rules:
      1: true
      5: false
    enabledExtensions:
      - ks-console-embed
      - kubeeye
    disabledExtensions:
      - whizard-telemetry
    maxDecodedBytes: 20971520
    maxConcurrent: 4
    maxQueued: 32
```

`client.basePath` is treated as an opaque, validated URL path. The sidecar does not extract a region name from it. `rewriteSidecar.upstream` is restricted to an explicit loopback HTTP address.

`rules` uses the numeric rule IDs in the table above. Every rule defaults to
`true`, including entries omitted from the map and configurations that omit
`rules` entirely. Set a rule to `false` to disable it or `true` to enable it.
Only IDs `1` through `6` are accepted.

Rules 2/3 and 4/5 overlap intentionally. If rule 3 is disabled while rule 2 is
enabled, Frontend Index JSBundle requests fall back to rule 2 without its
`text/plain` compatibility. If rule 5 is disabled while rule 4 is enabled,
ys1000 HTML falls back to the generic Named Proxy HTML rewrite.

`enabledExtensions` has three modes:

- `[]` disables the extension-controlled rules 1–3.
- A list of extension names enables only those extensions.
- `["*"]` enables every extension whose name matches
  `[A-Za-z0-9][A-Za-z0-9._-]{0,127}`.

Quote `"*"` so YAML treats it as a string. The wildcard cannot be combined with
explicit extension names, and partial glob patterns such as `ks-*` are not
supported. Wildcard mode changes only the extension allowlist: all request path,
method, asset type, and response eligibility checks described above still apply.

`disabledExtensions` is an optional list of explicit extension names. It always
takes precedence over `enabledExtensions`, including when
`enabledExtensions: ["*"]`. Disabled names must use the same safe extension-name
format, must be unique, and cannot contain `"*"` or partial glob patterns.

When the active rewrite limit is reached, up to `maxQueued` requests wait. A full queue receives `503` with `Retry-After: 1`.

## Response semantics

Rewritten responses remove length, digest, range, and upstream representation validators that no longer describe the emitted body. A weak ETag is derived from the upstream ETag, base path, extension, and rewrite rule version. Without a reliable upstream ETag, the response uses `Cache-Control: no-store`. Rewritten assets do not support byte ranges.

In wildcard mode, Prometheus metrics use `extension="*"` to keep label
cardinality bounded. Structured request logs continue to record the actual
extension name.

## Local development

Install [Lefthook](https://lefthook.dev/installation/) and enable the repository hooks once:

```bash
lefthook install
```

Commits run formatting and Clippy checks in parallel, then validate the commit
message against the Conventional Commits format. Pushes run the complete test
suite.

Start a KSE Console BFF on port `18000`, copy the example config, change the sidecar upstream to `http://127.0.0.1:18000`, and run:

```bash
KSE_REWRITE_CONFIG=examples/config.yaml cargo run
```

Then access the Console through `http://127.0.0.1:8080`. Internal endpoints are served only by `http://127.0.0.1:9090`:

- `/healthz`: process liveness.
- `/readyz`: loopback BFF connectivity.
- `/version`: package version, rewrite rule version, and build Git commit.
- `/metrics`: low-cardinality Prometheus metrics.

Query the running build with `wget`:

```bash
wget -qO- http://127.0.0.1:9090/version
```

```json
{"packageVersion":"0.1.0","rewriteRuleVersion":"v33","gitCommit":"0123456789abcdef0123456789abcdef01234567"}
```

The response uses `Cache-Control: no-store`. CI injects the full Git commit SHA.
Local builds report `"gitCommit":"unknown"` unless `KSE_GIT_COMMIT` is set at
compile time.

The Kubernetes example names `9090` as `admin-http` for direct Pod probes. Do not add that port to the Console Service or external Gateway. If Prometheus uses Pod discovery, restrict access with the cluster's monitoring/network policy.

## Deployment and rollback

Build the container with:

```bash
docker build --build-arg KSE_GIT_COMMIT="$(git rev-parse HEAD)" \
  -t <registry>/kse-static-rewrite-proxy:0.1.0 .
```

[deploy/sidecar-example.yaml](deploy/sidecar-example.yaml) is an illustrative
strategic-merge template, not a standalone `kubectl apply` manifest. Copy its
container changes into the real Console Deployment (or reference it from a
Kustomization as a patch), and adapt the Deployment name, ConfigMap volume,
labels, and image registry.

For a canary, create a separate one-replica Console Deployment with both containers and a unique label such as `rollout: rewrite-canary`. Create a canary Service selecting only that label and route a test hostname, header match, or small weighted share to it. Do not change the stable Service yet. Validate JS/CSS/JSON/HTML assets, binary bypass, authentication, APIs, WebSockets, SSE, cache revalidation, and queue metrics.

After the canary passes, add the sidecar to every stable Console Pod and wait for both BFF and sidecar readiness. Only then switch the stable Service `targetPort` from the BFF port to `console-http`; this avoids routing to old Pods that do not have the named sidecar port. Rollback is routing-first: restore the stable Service target to the BFF port, verify traffic, then remove the sidecar containers. Keep the BFF port unchanged throughout the rollout so this switch remains immediate.
