# KSE static rewrite proxy

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

### 当前重写规则统计（v29）

当前共有 **3 个顶层请求路径根、6 个响应处理规则**。请求路径根分别是
`extensions-static`、`jsbundles` 和 `proxy`；响应处理规则以代码中的
`RewriteProfile` 为统计口径。同一处理规则内部为了兼容不同构建产物而包含的
多种字符串形态，不重复计数。

| # | 处理规则 | 请求路径 | 响应要求 | 内容修改 |
|---|---|---|---|---|
| 1 | Console V3 静态资源 | `{basePath}/extensions-static/{extension}/dist/v3dist/**` | 已启用且未禁用的扩展；文件后缀为 `.js`、`.mjs`、`.css`、`.json`、`.html` 或 `.htm`；支持的 UTF-8 文本类型 | 为扩展静态资源根路径添加 `basePath`，并兼容独立 `/extensions-static/`、React Router `basename` 和相对 API URL 规范化逻辑 |
| 2 | JSBundle | `{basePath}/jsbundles/{extension}/dist/{distribution}/*.js` | 已启用且未禁用的扩展；`distribution` 等于 `extension`，或外层为 `{name}-frontend` 时等于 `{name}`；只匹配当前目录的直接 `.js` 文件 | 将 `` `//${window.location.host}/ `` 改为 `` `//${window.location.host}{basePath}/ `` |
| 3 | Frontend Index JSBundle | `{basePath}/jsbundles/{name}-frontend/dist/{name}-frontend/index.js` 或 `.../dist/{name}/index.js` | 已启用且未禁用的扩展；除标准 JavaScript 类型外，额外兼容未声明 charset 或 charset 为 UTF-8/UTF8 的 `text/plain` | 内容修改与 JSBundle 相同；这是独立的 Content-Type 兼容规则 |
| 4 | Named Proxy HTML | `{basePath}/proxy/{name}/` 及其任意子路径 | 仅处理 `text/html` 或 `application/xhtml+xml`；其他类型原样旁路 | 将小写、等号两侧无空白的 `href="/proxy/{name}/..."`、`src="/proxy/{name}/..."`（单双引号均可）改为带 `basePath` 的地址；Kubekey HTML 还会将固定旧根路径 `/57516e69-2cb0-4d48-a8a8-2833cfff87a9` 替换为 `basePath` |
| 5 | Named Proxy JavaScript | `{basePath}/proxy/{name}/**/*.js` | 标准 UTF-8 文本类型；不受扩展白名单控制 | 通常将固定的 `/proxy/{name}` 改为 `{basePath}/proxy/{name}`；Kubekey 仅替换完整的双引号字符串 `"/proxy/kubekey"` |
| 6 | Kubekey Assets JavaScript | `{basePath}/proxy/kubekey/assets/**/*.js` | 标准 UTF-8 文本类型；不受扩展白名单控制 | 将完整的双引号字符串 `"/proxy/kubekey"` 和固定旧根路径 `/57516e69-2cb0-4d48-a8a8-2833cfff87a9` 替换为对应的 `basePath` 地址，并将 `/kapis/kubekey.kubesphere.io` 添加 `basePath`；更长的 `/proxy/kubekey/...` 字符串不匹配 |

公共行为：

- 仅 `GET`、`HEAD` 请求参与规则匹配；`basePath` 为空时全部旁路。
- 只有 Console V3、JSBundle 和 Frontend Index JSBundle 受
  `enabledExtensions` / `disabledExtensions` 控制；Named Proxy 相关规则不受
  扩展白名单控制。
- 上游非 `200` 响应不修改。选中的响应要求使用 identity 编码，正文重写支持任意
  HTTP 分块边界并保持幂等。
- 由 Named Proxy HTML 规则选中的候选响应，如果最终 Content-Type 不是
  HTML/XHTML，则不会返回 502，并保留 Range 与条件缓存请求语义；图片、字体、
  视频等二进制资源的正文内容不修改，但候选请求仍会向上游请求 identity 编码。
- Named Proxy JavaScript 的优先级高于通用 HTML；Kubekey assets JavaScript
  的优先级又高于普通 Named Proxy JavaScript。

### 完整规则明细

#### 1. Console V3 静态资源

路径必须匹配
`{basePath}/extensions-static/{extension}/dist/v3dist/**`，扩展必须启用且未禁用，
最终文件名必须以 `.js`、`.mjs`、`.css`、`.json`、`.html` 或 `.htm`
结尾。选中的响应包含以下兼容处理：

1. 扩展专属根路径：

   ```text
   /extensions-static/{extension}/dist/v3dist/
           ->
   {basePath}/extensions-static/{extension}/dist/v3dist/
   ```

2. 独立静态根路径：

   ```text
   /extensions-static/
           ->
   {basePath}/extensions-static/
   ```

3. React Router `basename`：识别常见的压缩、非压缩和转义
   `basename: "".concat(..., "/consolev3")` 形式，在 `concat` 前加入
   `{basePath}/`。
4. API URL 规范化：为相对 URL 添加 `basePath`，已经带 `basePath` 的 URL
   保持不变。
5. 绝对 URL 保护：`http://`、`https://` 和协议相对 URL `//...` 不添加
   `basePath`。
6. 相对路径拼接兼容：处理构建产物中的普通、转义、返回语句及标识符表达式形式，
   避免重复 `/` 或重复添加 `basePath`。

这些兼容只作用于当前命中的扩展响应，不会扫描 API 响应或其他扩展资源。

#### 2. JSBundle

只匹配
`{basePath}/jsbundles/{extension}/dist/{distribution}/*.js` 当前目录下的直接
JavaScript 文件；`{extension}` 必须启用且未禁用。`distribution` 支持：

- 与外层 `{extension}` 完全相同。
- 当外层是 `{name}-frontend` 时，使用去掉 `-frontend` 的 `{name}`。

例如以下两种路径都匹配：

```text
.../jsbundles/ys1000-frontend/dist/ys1000-frontend/index.js
.../jsbundles/ks-autoscaling-frontend/dist/ks-autoscaling/index.js
```

其他任意不一致的 dist 名称以及更深层的 `.js` 文件不匹配。

```text
`//${window.location.host}/
        ->
`//${window.location.host}{basePath}/
```

固定扩展名和 JavaScript 插值形式均保留后续路径。已经带相同追加内容的模板字符串
不会再次添加。

#### 3. Frontend Index JSBundle

匹配已启用且未禁用的 `{name}-frontend` 外层扩展，其 dist 目录可以是完整的
`{name}-frontend`，也可以是去掉后缀的 `{name}`，文件名必须是 `index.js`。
内容修改与 JSBundle 相同。它额外接受 `text/plain`：可以不声明 charset；如果
声明，只接受 `utf-8` 或 `utf8`。

#### 4. Named Proxy HTML

匹配 `{basePath}/proxy/{name}/` 及其任意子路径，但只在最终响应为
`text/html` 或 `application/xhtml+xml` 时处理。HTML 属性必须同时满足：

- 属性名是小写 `href` 或 `src`，不匹配 `data-src`、`xlink:href` 等名称。
- 属性名前是 HTML ASCII 空白：空格、Tab、CR、LF 或 Form Feed。
- `=` 两侧没有空白，属性值使用单引号或双引号。
- 属性值以 `/proxy/{name}/` 开头；其他 name、相似前缀及绝对外部 URL 不匹配。

```text
href="/proxy/{name}/..."
src="/proxy/{name}/..."
        ->
href="{basePath}/proxy/{name}/..."
src="{basePath}/proxy/{name}/..."
```

当 `{name}` 为 `kubekey` 时，还会在 HTML 正文任意位置执行：

```text
/57516e69-2cb0-4d48-a8a8-2833cfff87a9
        ->
{basePath}
```

固定旧根路径之后的子路径保持不变。其他 Named Proxy HTML 不执行此替换。

最终响应不是 HTML/XHTML 时停止重写，正文内容不修改且不返回 502，同时保留
Range 和条件缓存请求语义。由于候选请求已经向上游请求 identity 编码，旁路响应
不保证保留原来的压缩表示。

#### 5. Named Proxy JavaScript

匹配 `{basePath}/proxy/{name}/**/*.js`，不受扩展白名单控制。

- 非 Kubekey：正文中的固定 `/proxy/{name}` 根路径添加 `basePath`，包括其后
  仍有子路径的字符串；已经紧邻相同 `basePath` 的输入不重复添加。
- Kubekey：只替换完整双引号字符串：

  ```text
  "/proxy/kubekey"
          ->
  "{basePath}/proxy/kubekey"
  ```

  `"/proxy/kubekey/..."`、`'/proxy/kubekey'` 和相似前缀不匹配。这里的
  “完整”指 JavaScript 字符串本身；在 `basePath + "/proxy/kubekey"` 表达式中，
  后面的双引号字符串仍然会匹配。

#### 6. Kubekey Assets JavaScript

匹配 `{basePath}/proxy/kubekey/assets/**/*.js`，优先于普通 Named Proxy
JavaScript，并同时执行：

```text
"/proxy/kubekey"
        ->
"{basePath}/proxy/kubekey"

/kapis/kubekey.kubesphere.io
        ->
{basePath}/kapis/kubekey.kubesphere.io

/57516e69-2cb0-4d48-a8a8-2833cfff87a9
        ->
{basePath}
```

第一条仍然只匹配完整双引号字符串；第二条匹配固定 KAPIS 根路径及其子路径。
第三条匹配固定旧根路径，其后的子路径保持不变。其他 KAPIS group 不修改。

### 响应类型与缓存规则

- 标准文本 Content-Type 白名单为 `text/javascript`、
  `application/javascript`、`application/x-javascript`、`text/css`、
  `application/json`、`text/json`、`text/html` 和
  `application/xhtml+xml`。
- charset 可以不声明；如果声明，只接受 `utf-8` 或 `utf8`。
- Frontend Index JSBundle 额外接受符合上述 charset 条件的 `text/plain`。
- Named Proxy HTML 只接受 HTML/XHTML；类型不匹配时旁路。其他已明确选中的规则
  遇到不支持的 Content-Type 时返回 502。
- 代理向上游请求 identity 编码；选中的响应仍带非 identity
  `Content-Encoding` 时返回 502。
- 成功改写会移除旧的长度、压缩、摘要、Range 和 Last-Modified 等表示元数据。
  上游 ETag 可靠时生成包含当前规则版本的新弱 ETag；否则返回
  `Cache-Control: no-store`。
- 所有正文替换都有最大解码字节限制，支持任意 HTTP 分块边界，并保证对已经处理的
  静态结果再次运行时保持幂等。
- 正文超过 `maxDecodedBytes` 或正文不是合法 UTF-8 时，正文重写失败并返回 502。

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

`enabledExtensions` has three modes:

- `[]` disables rewriting.
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
{"packageVersion":"0.1.0","rewriteRuleVersion":"v29","gitCommit":"0123456789abcdef0123456789abcdef01234567"}
```

The response uses `Cache-Control: no-store`. CI injects the full Git commit SHA.
Local builds report `"gitCommit":"unknown"` unless `KSE_GIT_COMMIT` is set at
compile time.

The Kubernetes example names `9090` as `admin-http` for direct Pod probes. Do not add that port to the Console Service or external Gateway. If Prometheus uses Pod discovery, restrict access with the cluster's monitoring/network policy.

## Deployment and rollback

Build the container with
`docker build --build-arg KSE_GIT_COMMIT="$(git rev-parse HEAD)" -t <registry>/kse-static-rewrite-proxy:0.1.0 .`.
[deploy/sidecar-example.yaml](deploy/sidecar-example.yaml) is an illustrative
strategic-merge template, not a standalone `kubectl apply` manifest. Copy its
container changes into the real Console Deployment (or reference it from a
Kustomization as a patch), and adapt the Deployment name, ConfigMap volume,
labels, and image registry.

For a canary, create a separate one-replica Console Deployment with both containers and a unique label such as `rollout: rewrite-canary`. Create a canary Service selecting only that label and route a test hostname, header match, or small weighted share to it. Do not change the stable Service yet. Validate JS/CSS/JSON/HTML assets, binary bypass, authentication, APIs, WebSockets, SSE, cache revalidation, and queue metrics.

After the canary passes, add the sidecar to every stable Console Pod and wait for both BFF and sidecar readiness. Only then switch the stable Service `targetPort` from the BFF port to `console-http`; this avoids routing to old Pods that do not have the named sidecar port. Rollback is routing-first: restore the stable Service target to the BFF port, verify traffic, then remove the sidecar containers. Keep the BFF port unchanged throughout the rollout so this switch remains immediate.
