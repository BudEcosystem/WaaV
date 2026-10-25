# Deployment Guide

WaaV Gateway ships as a single Axum binary that can be deployed anywhere you can run containers. This guide covers the recommended workflow for packaging, configuring, and running the service alongside LiveKit in Docker-based stacks (Docker Compose, Kubernetes, or any platform that can schedule OCI images).

## 1. Prerequisites

- **Docker with BuildKit** enabled (default in Docker 23.0+). Required for the Dockerfile cache mounts.
- **LiveKit cluster** reachable from the WaaV Gateway pod/container (LAN or VPC is ideal). Set `LIVEKIT_URL` to the internal address and `LIVEKIT_PUBLIC_URL` to the address clients should use.
- **Provider credentials** for the STT/TTS services you plan to use:
  - `DEEPGRAM_API_KEY` for Deepgram STT/TTS.
  - `ELEVENLABS_API_KEY` for ElevenLabs TTS.
- Optional: S3-compatible bucket for LiveKit recording egress (`RECORDING_S3_*` variables).
- Optional: Authentication settings documented in `docs/authentication.md`.
- Persistent volume (or host path) for `CACHE_PATH` when you want voice outputs and turn-detector assets to survive container restarts.

## 2. Building the Container Image

The provided multi-stage `Dockerfile` uses [cargo-chef](https://github.com/LukeMathWalker/cargo-chef) for optimized dependency caching and produces a minimal [distroless](https://github.com/GoogleContainerTools/distroless) runtime image. BuildKit is required.

```bash
docker build -t waav-gateway:latest .
```

### Build Stages

| Stage | Base Image | Purpose |
|-------|------------|---------|
| `chef` | `rust:${RUST_VERSION}-slim-bookworm` | Installs cargo-chef |
| `planner` | chef | Generates dependency recipe |
| `builder` | chef | Compiles release binary with cached deps |
| `init` | `debian:bookworm-slim` | Runs `waav-gateway init` to download models |
| `runtime` | `gcr.io/distroless/cc-debian12` | Minimal production image (~20MB base) |

### Build Arguments

| Argument | Default | Description |
|----------|---------|-------------|
| `RUST_VERSION` | `1.88.0` | Rust toolchain version (must support Rust 2024 edition) |
| `CARGO_BUILD_FEATURES` | `--all-features` | Feature flags. Use `--features turn-detect` or `""` for smaller builds |
| `ONNX_VERSION` | `1.23.2` | ONNX Runtime version for turn detection |
| `RUN_SAYNA_INIT` | `true` | Pre-download turn-detection assets. Set `false` to skip |

Example with custom args:

```bash
docker build -t waav-gateway:latest \
  --build-arg RUST_VERSION=1.88.0 \
  --build-arg CARGO_BUILD_FEATURES="--features turn-detect" \
  --build-arg RUN_SAYNA_INIT=false \
  .
```

### BuildKit Cache

The Dockerfile uses BuildKit cache mounts for the Cargo registry and git dependencies. These caches persist across builds and can be pruned with:

```bash
docker builder prune --filter type=exec.cachemount
```

### Distroless Runtime Notes

The production image uses Google's distroless base for minimal attack surface (~20MB). Key implications:

- **No shell**: You cannot `docker exec -it <container> sh`. Use `docker logs` for debugging.
- **No package manager**: All dependencies are copied at build time.
- **Debug variant**: For troubleshooting, you can temporarily switch to `debian:bookworm-slim` in the Dockerfile's runtime stage.

Default exposed port is `3001`. Override with `-e PORT=XXXX` if necessary.

## 3. Runtime Environment Variables

Add the relevant variables before starting the container. The most common ones are listed below.

| Variable | Purpose | Example |
| --- | --- | --- |
| `HOST` | Bind address inside the container. Usually leave as `0.0.0.0`. | `0.0.0.0` |
| `PORT` | Axum listener port. | `3001` |
| `CACHE_PATH` | Directory that stores cached audio and turn-detect assets. Mount a volume for persistence. | `/data/cache` |
| `DEEPGRAM_API_KEY` | Enables Deepgram STT/TTS. | `dg-secret` |
| `ELEVENLABS_API_KEY` | Enables ElevenLabs TTS. | `el-secret` |
| `LIVEKIT_URL` | Server-to-server WebSocket URL (internal). | `ws://livekit:7880` |
| `LIVEKIT_PUBLIC_URL` | URL clients should dial (returned via APIs). | `https://rtc.yourdomain.com` |
| `LIVEKIT_API_KEY` / `LIVEKIT_API_SECRET` | Credentials used to mint LiveKit tokens in `/livekit/token` and during WebSocket LiveKit bring-up. | `lk_key` / `lk_secret` |
| `WAAV_LIVEKIT_ICE_IPS` | Keep WaaV's LiveKit media sockets (ICE candidates) on addresses in these CIDR prefixes. Unset: every interface, plus loopback when `LIVEKIT_URL` is loopback. See section 7. | `127.0.0.1/32` |
| `RECORDING_S3_*` | Bucket configuration for LiveKit recordings (bucket, region, endpoint, access key, secret key). | `recordings`, `us-east-1`, etc. |

Set `AUTH_REQUIRED` plus either `AUTH_API_SECRETS_JSON` (preferred) or `AUTH_SERVICE_URL`/`AUTH_SIGNING_KEY_PATH` only if you follow the separate authentication guide. Legacy `AUTH_API_SECRET` is still supported with optional `AUTH_API_SECRET_ID`.

Minimal multi-secret example:

```bash
AUTH_API_SECRETS_JSON='[{"id":"default","secret":"sk_test_default_123"}]'
```

Your vendor keys are sent only to their vendors' hosts, and to hosts you approve. To approve other
hosts (a proxy, a local OpenAI-compatible server), use `WAAV_CREDENTIALS_JSON` /
`WAAV_CREDENTIALS_FILE` and `WAAV_PRIVATE_EGRESS_ALLOWLIST`. `WAAV_CLIENT_ENDPOINTS=deny` stops
clients from choosing endpoints at all. See [Egress and operator credentials](egress-and-credentials.md).

## 4. Local Docker Run

```bash
docker run --rm \
  -p 3001:3001 \
  -e HOST=0.0.0.0 \
  -e PORT=3001 \
  -e CACHE_PATH=/data/cache \
  -e DEEPGRAM_API_KEY=dg-secret \
  -e ELEVENLABS_API_KEY=el-secret \
  -e LIVEKIT_URL=ws://livekit:7880 \
  -e LIVEKIT_PUBLIC_URL=https://rtc.localhost \
  -e LIVEKIT_API_KEY=lk_key \
  -e LIVEKIT_API_SECRET=lk_secret \
  -v waav-gateway-cache:/data/cache \
  waav-gateway:latest
```

Mounting `waav-gateway-cache` ensures cached voices and turn detection assets persist.

## 5. Docker Compose Example

```yaml
# docker-compose.yml
version: "3.9"
services:
  livekit:
    image: livekit/livekit-server:latest
    environment:
      - LIVEKIT_KEYS=lk_key:lk_secret
    ports:
      - "7880:7880"     # WebRTC / signaling
      - "7881:7881"     # TURN (optional)

  waav-gateway:
    image: waav-gateway:latest
    depends_on:
      - livekit
    ports:
      - "3001:3001"
    environment:
      HOST: 0.0.0.0
      PORT: 3001
      CACHE_PATH: /data/cache
      DEEPGRAM_API_KEY: ${DEEPGRAM_API_KEY}
      ELEVENLABS_API_KEY: ${ELEVENLABS_API_KEY}
      LIVEKIT_URL: ws://livekit:7880
      LIVEKIT_PUBLIC_URL: https://rtc.example.com
      LIVEKIT_API_KEY: lk_key
      LIVEKIT_API_SECRET: lk_secret
      RECORDING_S3_BUCKET: waav-gateway-egress
      RECORDING_S3_REGION: us-east-1
      RECORDING_S3_ENDPOINT: https://s3.amazonaws.com
      RECORDING_S3_ACCESS_KEY: ${S3_ACCESS_KEY}
      RECORDING_S3_SECRET_KEY: ${S3_SECRET_KEY}
    volumes:
      - waav-gateway-cache:/data/cache

volumes:
  waav-gateway-cache: {}
```

The Compose network ensures `LIVEKIT_URL=ws://livekit:7880` resolves internally while `LIVEKIT_PUBLIC_URL` remains the externally reachable URL.

## 6. Kubernetes Deployment Example

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: waav-gateway
spec:
  replicas: 2
  selector:
    matchLabels:
      app: waav-gateway
  template:
    metadata:
      labels:
        app: waav-gateway
    spec:
      containers:
        - name: waav-gateway
          image: ghcr.io/your-org/waav-gateway:latest
          ports:
            - containerPort: 3001
          envFrom:
            - secretRef:
                name: waav-gateway-secrets   # API keys, LiveKit credentials
            - configMapRef:
                name: waav-gateway-config    # Non-sensitive values
          volumeMounts:
            - name: cache
              mountPath: /data/cache
      volumes:
        - name: cache
          persistentVolumeClaim:
            claimName: waav-gateway-cache-pvc
---
apiVersion: v1
kind: Service
metadata:
  name: waav-gateway
spec:
  selector:
    app: waav-gateway
  ports:
    - name: http
      port: 3001
      targetPort: 3001
  type: ClusterIP
```

Recommended ConfigMap entries:

```yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: waav-gateway-config
data:
  HOST: "0.0.0.0"
  PORT: "3001"
  CACHE_PATH: "/data/cache"
  LIVEKIT_URL: "ws://livekit.livekit.svc.cluster.local:7880"
  LIVEKIT_PUBLIC_URL: "https://rtc.example.com"
```

Store `DEEPGRAM_API_KEY`, `ELEVENLABS_API_KEY`, `LIVEKIT_API_KEY`, `LIVEKIT_API_SECRET`, and S3 credentials in `waav-gateway-secrets`.

## 7. LiveKit Configuration Walkthrough

1. **Networking**: Ensure the WaaV Gateway container can reach the LiveKit signaling endpoint specified in `LIVEKIT_URL`. In Compose, use the service name; in Kubernetes, use the service DNS name.
2. **Credentials**: Provide `LIVEKIT_API_KEY`/`LIVEKIT_API_SECRET`. WaaV Gateway uses these internally to mint:
   - Agent tokens during WebSocket configuration (`livekit` block of the `config` message).
   - User tokens via the `/livekit/token` REST endpoint.
3. **Client workflow**:
   - Call `/ws`, send a `config` payload that includes the `livekit` section:
     ```json
     {
       "type": "config",
       "stream_id": "support-room-42-2024-01-31",
       "audio": true,
       "stt_config": { "...": "..." },
       "tts_config": { "...": "..." },
       "livekit": {
         "room_name": "support-room-42",
         "enable_recording": true,
         "waav-gateway_participant_identity": "waav-gateway-ai",
         "waav-gateway_participant_name": "WaaV Gateway AI",
         "listen_participants": ["agent-1", "customer-42"]
       }
     }
     ```
   - Receive `ready` with `livekit_room_name`, `livekit_url`, and the WaaV Gateway participant metadata.
   - For each human participant, request a token via `POST /livekit/token` and join the LiveKit room directly.
4. **Media sockets (ICE)**: WaaV's participant opens its WebRTC media sockets on the host's
   interfaces. By default that's every interface (the LAN, each Docker bridge, VPNs, IPv6), plus
   loopback when `LIVEKIT_URL` points at loopback. To keep them on chosen addresses, set
   `WAAV_LIVEKIT_ICE_IPS` to CIDR prefixes. Any interface whose address is in one of them is used;
   no other is.
   - A single machine with everything on loopback: `WAAV_LIVEKIT_ICE_IPS=127.0.0.1/32`. LiveKit
     must offer loopback candidates too: `rtc.node_ip: 127.0.0.1`,
     `rtc.ips.includes: [127.0.0.1/32]` and `rtc.enable_loopback_candidate: true`.
   - A LAN deployment that shouldn't use Docker bridges or VPNs: the LAN prefix, for example
     `192.168.1.0/24`.

   A malformed value gathers no candidates at all (it fails closed), so LiveKit calls fail
   rather than quietly using every interface. `waav-gateway check` reports it. The setting is
   process-wide, because WebRTC's network stack is.

## 8. Configuration Variations

### A. Voice Pipeline Without LiveKit
- Leave all `LIVEKIT_*` variables unset and omit the `livekit` block in the WebSocket `config` message.
- Useful for pure WebSocket-only deployments or standalone REST TTS flows.

### B. LiveKit Mirroring With Recording
- Provide `LIVEKIT_URL`, `LIVEKIT_PUBLIC_URL`, `LIVEKIT_API_KEY`, `LIVEKIT_API_SECRET`.
- Configure `RECORDING_S3_*` so LiveKit recording egress can persist files.
- Set `"enable_recording": true` and include a session-level `stream_id` to control the `{server_prefix}/{stream_id}/audio.ogg` recording path. Omit `stream_id` to let the server generate one.

### C. Text/Data-Only Sessions
- Start WaaV Gateway with normal audio credentials but send a WebSocket `config` message where `"audio": false`.
- Lets you keep LiveKit messaging/data-plane behavior (including `/livekit/token`) while skipping STT/TTS provider initialization, ideal for environments without audio API keys.

### D. Custom Cache Strategies
- **Filesystem cache** (recommended for production): set `CACHE_PATH` to a mounted volume so `waav-gateway init` persists turn-detect assets and TTS audio across restarts.
- **In-memory cache**: omit `CACHE_PATH` to keep assets in RAM; faster to bootstrap but cleared on restarts.

### E. Feature Flag Tuning
- Default builds enable no optional features. Add only what you need:
  - `cargo run` (no optional features)
  - `cargo run --features turn-detect` (turn detection enabled)
  - `cargo run --features openapi` (OpenAPI enabled)
- Override feature flags at build time: `--build-arg CARGO_BUILD_FEATURES="--features turn-detect"`.
- The published Dockerfile defaults to `--no-default-features --features turn-detect,noise-filter` (OpenAPI off). For smaller images, override the build args:
  ```bash
  docker build -t waav-gateway:minimal --build-arg CARGO_BUILD_FEATURES="--no-default-features" .
  ```

## 9. Checking a Configuration

`waav-gateway check -c config.yaml [--env-file .env]` runs the start-up checks without starting the
server, loading models or dialling anything. It checks:

- the env file and the config file, including **unknown keys** (a typo such as `securty:` would
  otherwise be ignored and the defaults used);
- operator credentials and DAG templates;
- the auth, metrics and segmented speech-to-text settings.

Each problem is printed, and the exit status is 1 when there is one. When the server itself
starts, an unknown key is logged as a warning rather than refused, so existing files keep starting.

## 10. Probes and the Operator Listener

| Path | Answers | Notes |
|---|---|---|
| `/livez` | process up | never touches an upstream |
| `/ready` | Bud control plane hydrated; `503 draining` after SIGTERM | the Bud chart's probe |
| `/readyz` | provider credentials present and reachable | `503` names the unready provider |
| `/metrics` | Prometheus | see `WAAV_METRICS_TOKEN` / `WAAV_METRICS_PUBLIC` |

How `/readyz` probes:

- It probes the address each provider's traffic actually goes to. For the openai providers that
  is `OPENAI_BASE_URL` when it is set. `WAAV_READINESS_PROBE_<PROVIDER>=host:port` overrides it.
- One evaluation answers every request for 10 seconds, and simultaneous requests share it, so
  polling `/readyz` does not multiply outbound connections.
- `WAAV_READINESS_SKIP_NETWORK=1` checks credentials only.

`WAAV_METRICS_ADDR=host:port` opens an operator listener on an address your ingress does not
route to. It serves `/metrics`, `/livez`, `/ready` and `/readyz`. With it set, the public `/readyz`
returns its verdict without the per-provider breakdown, which would otherwise tell anyone which
vendor credentials the gateway holds. The full breakdown stays on the operator listener. All
probes remain on the public port, so existing liveness and readiness probes keep working.

### A Unix socket for local callers

`WAAV_UNIX_SOCKET=/run/waav/waav.sock` also serves the whole API on a Unix socket, for callers on the
same host that address services by path. How it behaves:

- **Auth.** It is required exactly as on TCP.
- **No rate limiter.** Every caller on the socket would share one bucket.
- **Connection cap.** The per-peer connection cap counts all socket callers as one peer.
- **Permissions.** The socket is created `0660`; its file permissions decide who may connect.
- **Stale files.** A socket left by a crashed run is reused. Anything else at the path is
  refused, never deleted. The socket is removed on a graceful stop.

```sh
curl --unix-socket /run/waav/waav.sock -H "Authorization: Bearer $KEY" http://localhost/voices
```

## 11. Verification Checklist

1. `curl http://<waav-gateway-host>:3001/` returns `{ "status": "OK" }`.
2. `GET /voices` succeeds (requires provider keys).
3. WebSocket `config` message with LiveKit block yields a `ready` payload that includes `livekit_room_name`.
4. `POST /livekit/token` issues tokens that successfully join the LiveKit room.
5. If recordings are enabled, verify the output object lands in the configured S3 bucket.

Following this guide you can deploy WaaV Gateway in any container-friendly environment, pairing it with LiveKit for low-latency, bidirectional speech workflows.
