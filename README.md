# KSE static rewrite proxy

A temporary, independently deployable Pingora sidecar for KSE Console. It forwards every Console request to the BFF unchanged, except for a narrowly scoped stream rewrite in configured extension V3 text assets.

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

A response is eligible only when all conditions hold:

- Request method is `GET` or `HEAD`.
- Request path is `{basePath}/extensions-static/{extension}/dist/v3dist/**`.
- `{extension}` is in `rewriteSidecar.rewrite.enabledExtensions`.
- Asset suffix is `.js`, `.mjs`, `.css`, `.json`, `.html`, or `.htm`.
- Upstream returns `200`, a supported UTF-8 text `Content-Type`, and no content encoding after the sidecar requests `identity`.

The response body is rewritten as a stream:

```text
/extensions-static/{extension}/dist/v3dist/
        ->
{basePath}/extensions-static/{extension}/dist/v3dist/
```

The operation is idempotent across arbitrary HTTP chunk boundaries. Fonts, images, and all other binary assets bypass the rewrite and retain their normal upstream compression.

For eligible text assets the sidecar asks the BFF for an identity response, performs the bounded literal rewrite, then lets Pingora negotiate downstream gzip, Brotli, or Zstandard compression from the browser's original `Accept-Encoding` header.

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
    maxDecodedBytes: 20971520
    maxConcurrent: 4
    maxQueued: 32
```

`client.basePath` is treated as an opaque, validated URL path. The sidecar does not extract a region name from it. `rewriteSidecar.upstream` is restricted to an explicit loopback HTTP address.

When the active rewrite limit is reached, up to `maxQueued` requests wait. A full queue receives `503` with `Retry-After: 1`.

## Response semantics

Rewritten responses remove length, digest, range, and upstream representation validators that no longer describe the emitted body. A weak ETag is derived from the upstream ETag, base path, extension, and rewrite rule version. Without a reliable upstream ETag, the response uses `Cache-Control: no-store`. Rewritten assets do not support byte ranges.

## Local development

Start a KSE Console BFF on port `18000`, copy the example config, change the sidecar upstream to `http://127.0.0.1:18000`, and run:

```bash
KSE_REWRITE_CONFIG=examples/config.yaml cargo run
```

Then access the Console through `http://127.0.0.1:8080`. Internal endpoints are served only by `http://127.0.0.1:9090`:

- `/healthz`: process liveness.
- `/readyz`: loopback BFF connectivity.
- `/metrics`: low-cardinality Prometheus metrics.

The Kubernetes example names `9090` as `admin-http` for direct Pod probes. Do not add that port to the Console Service or external Gateway. If Prometheus uses Pod discovery, restrict access with the cluster's monitoring/network policy.

## Deployment and rollback

Build the container with `docker build -t <registry>/kse-static-rewrite-proxy:0.1.0 .`. [deploy/sidecar-example.yaml](deploy/sidecar-example.yaml) is an illustrative strategic-merge template, not a standalone `kubectl apply` manifest. Copy its container changes into the real Console Deployment (or reference it from a Kustomization as a patch), and adapt the Deployment name, ConfigMap volume, labels, and image registry.

For a canary, create a separate one-replica Console Deployment with both containers and a unique label such as `rollout: rewrite-canary`. Create a canary Service selecting only that label and route a test hostname, header match, or small weighted share to it. Do not change the stable Service yet. Validate JS/CSS/JSON/HTML assets, binary bypass, authentication, APIs, WebSockets, SSE, cache revalidation, and queue metrics.

After the canary passes, add the sidecar to every stable Console Pod and wait for both BFF and sidecar readiness. Only then switch the stable Service `targetPort` from the BFF port to `console-http`; this avoids routing to old Pods that do not have the named sidecar port. Rollback is routing-first: restore the stable Service target to the BFF port, verify traffic, then remove the sidecar containers. Keep the BFF port unchanged throughout the rollout so this switch remains immediate.
