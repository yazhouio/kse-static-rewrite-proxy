# KSE 静态资源重写代理

[English](README.md) | [简体中文](README.zh-CN.md)

这是一个为 KSE Console 提供的临时、可独立部署的 Pingora sidecar。它将 Console
请求转发到 BFF，为缺少 `basePath` 的 API 请求路径补齐前缀，并仅对已配置扩展的
静态资源执行范围严格受限的流式重写。

## 请求流程

```text
Gateway / Ingress
       |
       v
Pingora sidecar :8080 ----> KSE Console BFF 127.0.0.1:8000
       |
       +--- admin :9090（仅 Pod 探针 / Prometheus）
```

Console Service 必须指向 sidecar 的 `8080` 端口；BFF 仍是同一 Pod 内部的上游。
除下述 API 路径兼容规则外，Sidecar 保留请求路径、Host、Cookie、登录/登出行为、
WebSocket、SSE 和上传。

健康检查与指标使用独立的 `9090` 管理端口。Console Service 只暴露 `8080`，因此
Console 的通配 Ingress 路由无法访问或遮蔽管理端点。

### API 请求路径兼容

请求转发到上游前，如果路径尚未以完整的 `basePath` 开头，并且包含 `/kapis/`
或 `/apis/`，Sidecar 会在路径前添加 `basePath`。查询参数保持不变；
`basePath` 为空时不修改请求。

```text
/apis/apps/v1/deployments?limit=20
        ->
{basePath}/apis/apps/v1/deployments?limit=20
```

## 重写范围

### 当前重写规则统计（v33）

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
| 5 | Ys1000 MIG Meta HTML | `{basePath}/proxy/ys1000/` 及其任意子路径 | 仅处理 `text/html` 或 `application/xhtml+xml`；其他类型原样旁路 | Base64 解码 `window._mig_meta` 中的 JSON，将顶层 `baseURI` 从 `/proxy/ys1000` 改为 `{basePath}/proxy/ys1000`，再按原位置重新编码；同时继承规则 4 的 HTML 属性修改 |
| 6 | Kubekey Assets JavaScript | `{basePath}/proxy/kubekey/assets/**/*.js` | 标准 UTF-8 文本类型；不受扩展白名单控制 | 仅将固定旧根路径 `/57516e69-2cb0-4d48-a8a8-2833cfff87a9` 替换为 `basePath` |

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
- 普通 Named Proxy JavaScript 不再重写；Kubekey assets JavaScript 是唯一的
  Named Proxy JavaScript 特例，并优先于通用 HTML。

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

#### 5. Ys1000 MIG Meta HTML

匹配 `{basePath}/proxy/ys1000/` 及其任意子路径，并继承规则 4 的 HTML
属性修改。响应正文中的赋值：

```text
window._mig_meta = '<Base64 JSON>';
```

会先使用标准 Base64 解码。若结果是 JSON，并且顶层 `baseURI` 的值恰好为
`/proxy/ys1000`，则改为：

```text
{basePath}/proxy/ys1000
```

修改后的 JSON 使用标准 Base64 重新编码并放回原赋值位置。赋值周围的 HTML、
空白和引号不变；无效 Base64、无效 JSON、缺少 `baseURI` 或其他 `baseURI`
值均保持原样。已经是目标值时不重复修改。

#### 6. Kubekey Assets JavaScript

匹配 `{basePath}/proxy/kubekey/assets/**/*.js`，并且只执行：

```text
/57516e69-2cb0-4d48-a8a8-2833cfff87a9
        ->
{basePath}
```

固定旧根路径之后的子路径保持不变。规则 6 不修改 `"/proxy/kubekey"` 或
`/kapis/kubekey.kubesphere.io`。其他 Named Proxy JavaScript 全部旁路。

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

## 配置

Sidecar 与 Console 读取同一个 YAML 文档。可通过 `KSE_REWRITE_CONFIG` 更改路径；
默认路径为 `/etc/kse-console/config.yaml`。

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

`client.basePath` 被视为一个不透明、经过校验的 URL 路径，sidecar 不会从中提取
region 名称。`rewriteSidecar.upstream` 仅允许显式的 loopback HTTP 地址。

`rules` 使用上表中的数字规则编号。所有规则默认都是 `true`；未配置 `rules` 或
省略某个编号时，该规则仍默认开启。设置 `false` 可关闭规则，设置 `true` 可开启
规则。只接受编号 `1` 到 `6`。

规则 2/3 和 4/5 有意重叠。关闭规则 3、开启规则 2 时，Frontend Index JSBundle
请求会回退到规则 2，但不再兼容 `text/plain`。关闭规则 5、开启规则 4 时，
ys1000 HTML 会回退到通用 Named Proxy HTML 重写。

`enabledExtensions` 有三种模式：

- `[]` 关闭受扩展列表控制的规则 1–3。
- 扩展名列表只开启指定扩展。
- `["*"]` 开启所有名称符合
  `[A-Za-z0-9][A-Za-z0-9._-]{0,127}` 的扩展。

请为 `"*"` 加引号，使 YAML 将其作为字符串处理。通配符不能与显式扩展名混用，
也不支持 `ks-*` 等部分 glob。通配模式只影响扩展白名单；上述请求路径、方法、
资源类型和响应资格检查仍全部生效。

`disabledExtensions` 是可选的显式扩展名列表，优先级始终高于
`enabledExtensions`，包括 `enabledExtensions: ["*"]`。禁用项必须使用相同的
安全扩展名格式、不得重复，也不能包含 `"*"` 或部分 glob。

达到活动重写并发上限时，最多有 `maxQueued` 个请求等待；队列已满时返回 `503`
并带 `Retry-After: 1`。

## 响应语义

重写后的响应会移除已经不能描述输出正文的长度、摘要、Range 和上游表示校验元数据。
如果上游 ETag 可靠，则根据上游 ETag、base path、扩展名和重写规则版本派生弱
ETag；否则响应使用 `Cache-Control: no-store`。重写后的资源不支持字节 Range。

通配模式下，Prometheus 指标使用 `extension="*"`，避免标签基数失控；结构化请求
日志仍记录实际扩展名。

## 本地开发

安装 [Lefthook](https://lefthook.dev/installation/) 并为仓库启用一次 hooks：

```bash
lefthook install
```

提交时会并行执行格式检查和 Clippy，然后校验 Conventional Commits 格式；推送时
执行完整测试套件。

在 `18000` 端口启动 KSE Console BFF，复制示例配置，将 sidecar 上游改为
`http://127.0.0.1:18000`，然后运行：

```bash
KSE_REWRITE_CONFIG=examples/config.yaml cargo run
```

通过 `http://127.0.0.1:8080` 访问 Console。以下内部端点仅由
`http://127.0.0.1:9090` 提供：

- `/healthz`：进程存活检查。
- `/readyz`：loopback BFF 连通性。
- `/version`：包版本、重写规则版本和构建 Git commit。
- `/metrics`：低基数 Prometheus 指标。

使用 `wget` 查询当前构建：

```bash
wget -qO- http://127.0.0.1:9090/version
```

```json
{"packageVersion":"0.1.0","rewriteRuleVersion":"v33","gitCommit":"0123456789abcdef0123456789abcdef01234567"}
```

该响应使用 `Cache-Control: no-store`。CI 注入完整 Git commit SHA；本地构建在
未设置 `KSE_GIT_COMMIT` 时报告 `"gitCommit":"unknown"`。

Kubernetes 示例将 `9090` 命名为 `admin-http`，供 Pod 探针直接访问。不要把该
端口加入 Console Service 或外部 Gateway。若 Prometheus 使用 Pod 服务发现，应
通过集群监控/网络策略限制访问。

## 部署与回滚

使用以下命令构建容器：

```bash
docker build --build-arg KSE_GIT_COMMIT="$(git rev-parse HEAD)" \
  -t <registry>/kse-static-rewrite-proxy:0.1.0 .
```

[`deploy/sidecar-example.yaml`](deploy/sidecar-example.yaml) 是示意性的 strategic
merge 模板，不能直接作为独立 manifest 执行 `kubectl apply`。请把其中的容器
变更复制到实际 Console Deployment，或在 Kustomization 中将其作为 patch 引用，
并调整 Deployment 名称、ConfigMap volume、标签和镜像仓库。

灰度发布时，创建一个单副本 Console Deployment，包含两个容器并使用独立标签，
例如 `rollout: rewrite-canary`。创建只选择该标签的 canary Service，并通过测试
域名、Header 匹配或小比例权重导入流量。先不要修改稳定 Service。验证 JS、CSS、
JSON、HTML 资源、二进制旁路、认证、API、WebSocket、SSE、缓存再验证和队列指标。

灰度验证通过后，将 sidecar 加入所有稳定 Console Pod，并等待 BFF 与 sidecar
都 Ready。随后再把稳定 Service 的 `targetPort` 从 BFF 端口切换到
`console-http`，避免流量被路由到尚无具名 sidecar 端口的旧 Pod。回滚时先恢复
稳定 Service 指向 BFF，确认流量后再删除 sidecar 容器。整个发布过程保持 BFF
端口不变，使切换可以立即完成。
