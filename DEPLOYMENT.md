# Split Taskboard deployment

Taskboard uses the public container as a native `rtn-mq` gateway. This is not WebAssembly: the browser talks normal same-origin HTTP to the gateway, and the gateway talks to the private backend through Iroh.

```text
browser --HTTPS--> gateway container --signed rtn-mq / Iroh--> backend container --> Turso
                    public DNS + TLS                                  no HTTP listener
```

Only `/api/v1` is tunneled. Static frontend assets and SPA routes stay at the gateway. Request and response bodies are streamed in 512 KiB frames, so image uploads are not constrained by `rtn-mq`'s 1 MiB per-message ceiling.

## What is authenticated

The backend seed command creates an Iroh endpoint identity and realm authority in Turso. Later join grants and issued gateway memberships are persisted in the same database. The gateway creates its own private endpoint key on first startup and persists it separately.

The join code is a bearer enrollment credential, but it is not used to sign application traffic. Successful enrollment binds the one permitted use to the gateway's public key. Every later `rtn-mq` message is signed by that key and checked against its certificate and exact topic permissions:

- Gateway: publish `taskboard/http/request`, subscribe `taskboard/http/response`.
- Backend: subscribe `taskboard/http/request`, publish `taskboard/http/response`.

Someone who gets the code after the legitimate gateway enrolls cannot register a different key. Someone who gets it before enrollment can race the gateway and consume the only use, so deliver it as a secret. Anyone who obtains the gateway's persisted private key as well as its code can impersonate that gateway; protect the gateway volume like any other service credential.

The join code may be supplied as `TASKBOARD_RTN_JOIN_CODE`. Container environment variables are acceptable when administrators and the container runtime are trusted, but they are visible through container inspection. `TASKBOARD_RTN_JOIN_CODE_FILE` is also supported for a mounted secret. The code is read only by the native gateway process and is never compiled into frontend assets or sent to a browser.

Browser-request protection is enforced at the gateway, before forwarding into the signed tunnel. For methods other than GET, HEAD, and OPTIONS, it rejects `Sec-Fetch-Site: cross-site` and any supplied `Origin` that does not match its configured `TASKBOARD_GATEWAY_ORIGIN`. Null, malformed, and duplicate origins are rejected. Requests without either browser header remain supported for non-browser clients; they still need the backend's session and CSRF credentials for authenticated writes. Login is subject to the gateway check too. This separates [browser CSRF protection](https://cheatsheetseries.owasp.org/cheatsheets/Cross-Site_Request_Forgery_Prevention_Cheat_Sheet.html) from gateway enrollment.

The gateway uses its configured public origin, not client-supplied `Host`, `Forwarded`, or `X-Forwarded-*` values, so TLS termination does not require trusting those headers. A reverse proxy must preserve the browser's `Origin` and `Sec-Fetch-Site` headers. The private backend has no browser-origin allowlist: it trusts enrolled gateways to perform this check, while independently enforcing user authentication, session-bound CSRF tokens, and permissions.

## Repository layout and builds

Install [just](https://just.systems/man/en/installation.html) and Bash on each host to use the repository’s `justfile` commands. Run `just --list` to see the available recipes.

The local builds expect adjacent checkouts:

```text
parent/
  rtn-mq/
  rtn-taskboard/
```

Compose passes `../rtn-mq` as a named Docker build context. That checkout must include the application storage APIs (`HostStorage`, `generate_host_state`, `host_with_storage`, and identity byte conversion) and `topics_ready`. Publish companion changes to the `rtn-mq` repository separately, then update both checkouts on the build host. A Taskboard-only pull cannot update this dependency.

To build without Compose:

```sh
docker build --build-context rtn_mq=../rtn-mq --build-arg TASKBOARD_UID="$(id -u)" --build-arg TASKBOARD_GID="$(id -g)" -f Dockerfile.backend -t taskboard-backend:local .
docker build --build-context rtn_mq=../rtn-mq --build-arg TASKBOARD_UID="$(id -u)" --build-arg TASKBOARD_GID="$(id -g)" -f Dockerfile.gateway -t taskboard-gateway:local .
```

In a registry-based deployment, build both images in CI and deploy only the appropriate image and Compose configuration to each machine.

`just` automatically exports the invoking user's UID/GID for both image builds and container runtime users. For direct Compose commands, run this in your shell on **each host** first (the numbers may differ between machines):

```sh
export TASKBOARD_UID="$(id -u)" TASKBOARD_GID="$(id -g)"
```

Compose requires these values rather than silently falling back to another user. Existing storage, including a gateway volume created from a registry image with different IDs, needs matching ownership; see [Changing the container user](README.md#changing-the-container-user). Normal services remain non-root when invoked by a non-root user.

All three Compose files use `network_mode: host`. On Linux, services and `docker compose run` commands share the host network namespace, avoiding the Compose bridge and its embedded DNS. Docker Desktop 4.34 or later requires **Settings → Resources → Network → Enable host networking** ([Docker documentation](https://docs.docker.com/engine/network/drivers/host/)). Direct Iroh connections with automatic relay fallback are enabled by default.

The gateway binds directly to `${TASKBOARD_PUBLISH_ADDRESS}:8080`, with `127.0.0.1` as the default address. There are no Docker port mappings; port 8080 must be free on the gateway host. After changing an existing deployment to host networking, apply it with `docker compose up -d --force-recreate` (use the relevant `-f` option for a split deployment). A container restart alone does not apply networking changes, and no image rebuild is needed.

### Direct connections and relay fallback

Keep `TASKBOARD_RTN_RELAY_ONLY=false` on **both** hosts. Iroh uses its default relays for discovery and initial connectivity, then moves traffic to a direct UDP path when one is reachable. If direct connectivity is unavailable or lost, the relay remains available as a fallback ([Iroh relay behavior](https://docs.iroh.computer/concepts/relays)). `true` is an explicit opt-in to force relay-only transport.

Existing `.env` values override the defaults. After changing this setting, recreate the appropriate service with `docker compose -f compose.backend.yaml up -d backend` or `docker compose -f compose.gateway.yaml up -d gateway`. Keep the existing join code and identity; changing transport does not require re-enrollment with a new key or code.

## 1. Prepare the backend server

Create `.env` from `.env.example`. The public URL on the private backend creates invitation, password-reset, and Discord links; it is not used to validate browser origins:

```dotenv
TASKBOARD_BASE_URL=https://tasks.example.com
TASKBOARD_SECURE_COOKIES=true
TASKBOARD_MAX_DB_GIB=0
TASKBOARD_MAX_IMAGE_MIB=10
TASKBOARD_RTN_RELAY_ONLY=false
```

The backend bind-mounts `./data:/data`, so its database is stored at `./data/taskboard.db` on this host and owned by your user. The backend recipes prepare this directory automatically. For a new installation using Compose directly, create it as your user before running backend commands:

```sh
umask 077
mkdir -p ./data
```

This Turso release requires a fresh database and a new gateway join code. Previous databases are not imported; see [Breaking storage change](README.md#breaking-storage-change).

Build the image, then seed the database while the backend service is stopped:

```sh
just backend
just seed "you@company.com" "Your Name"
```

`just seed` prompts for the editor password. It initializes the schema, then commits the initial editor, backend private key, and CBOR authority/enrollment state in `./data/taskboard.db` together. It runs offline, writes no backend `.key` or `.cbor` files, and refuses to overwrite existing users or identity state. Startup requires this seeded identity; it does not import old files or silently generate new credentials.

Issue the gateway code and start the backend:

```sh
just join-code
just start-backend
```

Keep the printed `rtn-mq://join/...` value private. Issuing a code requires relay connectivity and saves the grant in Turso. Run only one process against the database at a time. Stop the backend before any database CLI command, including `issue-gateway-code`, `status`, and `invalidate-sessions`.

`just start-backend` uses the image built by `just backend` and follows the backend logs. Press `Ctrl+C` to stop following logs; the service keeps running.

There is deliberately no `ports` entry and no HTTP listener inside the backend container. API requests can reach its in-process router only after arriving as authenticated tunnel frames. `TASKBOARD_RTN_RELAY_ONLY=false` enables direct UDP transport while retaining the default Iroh relays for discovery and fallback.

## 2. Prepare the gateway host

Create its `.env` with the code printed by the backend and the origin users will visit. This browser-origin setting belongs only to the gateway; backend and Discord secrets stay out of its container:

```dotenv
TASKBOARD_PUBLISH_ADDRESS=127.0.0.1
TASKBOARD_GATEWAY_ORIGIN=https://tasks.example.com
TASKBOARD_RTN_RELAY_ONLY=false
TASKBOARD_RTN_JOIN_CODE=rtn-mq://join/REPLACE_ME
```

Build and start it:

```sh
docker compose -f compose.gateway.yaml build
docker compose -f compose.gateway.yaml up -d
docker compose -f compose.gateway.yaml ps
docker compose -f compose.gateway.yaml logs -f gateway
```

Keep `taskboard-gateway-data`. Its private key is the identity that consumed the join code. Recreating only the container is safe; deleting this volume produces a new identity that the already-consumed code correctly rejects.

For hosts with ephemeral storage, the gateway also accepts
`TASKBOARD_RTN_IDENTITY_HEX`: the same 32-byte private key encoded as 64 hexadecimal
characters and supplied through a runtime secret manager. When present, this key
overrides `TASKBOARD_RTN_IDENTITY`; the gateway does not read or write an identity
file. Empty or malformed secrets fail startup instead of generating a new key.
Preserve both the secret and its matching join code across deployments. Keep the
private key out of source control and image builds, and run only one instance
with that identity. This option is supported when running the gateway image
directly; the existing Compose files continue to use their identity volume.

Configure the existing TLS reverse proxy to send `https://tasks.example.com` to `127.0.0.1:8080`. If the gateway container itself should be LAN-accessible, change `TASKBOARD_PUBLISH_ADDRESS`, but this has no effect on backend networking.

For local HTTP instead, set `TASKBOARD_GATEWAY_ORIGIN=http://localhost:8080` (the default) or `http://127.0.0.1:8080`, matching the address you actually open, and use `TASKBOARD_SECURE_COOKIES=false` on the backend. The backend link URL can differ without blocking login or writes.

When upgrading from backend-wide origin validation, rebuild and recreate both services on their respective machines. Set `TASKBOARD_GATEWAY_ORIGIN` on the gateway before upgrading if its browser URL is not the default; it does not inherit the backend's `TASKBOARD_BASE_URL`. Preserve the backend data directory, gateway volume, and existing join code.

## Recovery and operations

- Both processes prefer direct connections and use a relay when a direct path is unavailable. The gateway explicitly re-enrolls its same persisted identity after a backend restart; this does not consume another code use.
- Gateway readiness requires both tunnel topic subscriptions, not just a connected peer. A new API request waits up to eight seconds for subscriptions to recover, then returns `503` if they are still unavailable. Static assets remain available. The existing concurrency limit also bounds requests waiting for recovery.
- The gateway only retries a request-start publication when `rtn-mq` explicitly reports that no subscriber accepted it. Once a publication is accepted, acknowledgement failures do not trigger a fresh HTTP request or replay; the messaging layer retains its existing delivery semantics.
- A connection loss can make the outcome of an in-flight write unknown after the backend accepted its frames. `rtn-mq` prevents message forgery and duplicate frame delivery, but it does not turn Taskboard CRUD operations into a cross-process transaction. Reconcile state before manually repeating a write that ended in `503`.
- Back up the backend host's `./data` directory, including `taskboard.db` and Turso sidecars. The database contains the backend key and enrollment state. Back up the gateway `/data/rtn` identity independently.
- Never copy the gateway identity into a second running gateway. The current protocol intentionally supports one global gateway for this code and expects exactly one response recipient.
- Issuing another ordinary code does not revoke an existing gateway certificate or free its membership slot. Use the replacement procedure below if the original gateway identity is lost or compromised.

### Replacing a gateway

Docker Desktop and Colima have separate volume stores. Moving between them can create a new `/data/rtn/gateway.key` even when the Compose volume name is unchanged. The backend's one-use code still belongs to the old key, and `rtn-mq` currently reports its enrollment limit as `queue or memory budget exhausted`. This rejection can occur over a working relay connection. The Iroh warning `IPv4 address detected by QAD varies by destination` is a separate direct-address probe result and does not mean enrollment failed. Keep `TASKBOARD_RTN_RELAY_ONLY=false` on both services to permit direct connections with relay fallback.

If the original private gateway key is available, move it securely to the new gateway's volume with its private permissions and stop the old gateway. Otherwise, replace its enrollment on the backend host:

```sh
just stop-backend
just backend
just replace-gateway-code
just start-backend
```

`just replace-gateway-code` runs `taskboard issue-gateway-code --replace`. It invalidates **all previous gateway join codes and certificates** and frees the single gateway membership slot. It preserves the backend endpoint key, users, sessions, tasks, attachments, and other application data. The new authority and one-use grant are saved together; failure to connect to the relay or save the grant leaves the previous enrollment state intact. Run it only while the backend service is stopped.

On the gateway host, put the printed replacement code into `TASKBOARD_RTN_JOIN_CODE` in `.env`, keep the gateway's current private-key volume, and run `just start-gateway`. Preserve both this code and the gateway volume for future restarts. Do not run `seed` or delete the backend database to replace a gateway.

### Diagnosing disconnects

The logging defaults include peer connection/removal events, QUIC close reasons, session duration, control/data stream failures, protocol rejections, and queue/certificate failures. They do not log join secrets, private keys, browser headers, or request bodies. On reconnect, the gateway logs how long recovery took and whether both subscriptions became ready.

If an existing `.env` overrides `RUST_LOG`, include these filters on both hosts and recreate the corresponding container:

```dotenv
RUST_LOG=taskboard=info,taskboard_gateway=info,tower_http=info,rtn_mq=info,iroh=warn,iroh_relay=warn
```

An exported shell `RUST_LOG` overrides `.env` during Compose interpolation. Unset it (or use `env -u RUST_LOG` before the Compose command) if the container is still using an older filter.

Capture both sides over the same time window:

```sh
# Gateway host (export TASKBOARD_UID/GID first when using Compose directly)
docker compose -f compose.gateway.yaml logs --since=10m --timestamps gateway
# Backend host
docker compose -f compose.backend.yaml logs --since=10m --timestamps backend
```

Look for the earliest transport/protocol error preceding `messaging peer removed`, then correlate peer IDs and timestamps across hosts. `Taskboard backend disconnected; attempting to rejoin` is the recovery symptom, not the underlying cause. Local shutdowns and deliberate reconnect tests will also produce connection-close events.

## Local two-container deployment

`compose.yaml` runs both roles with host networking on one Docker host and defaults to direct connections with relay fallback. Follow the README quick start. Application traffic still crosses the authenticated Iroh tunnel; the backend has no HTTP listener, and neither service uses Docker port mappings.
