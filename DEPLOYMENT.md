# Split Taskboard deployment

Taskboard uses the public container as a native `rtn-mq` gateway. This is not WebAssembly: the browser talks normal same-origin HTTP to the gateway, and the gateway talks to the private backend through Iroh.

```text
browser --HTTPS--> gateway container --signed rtn-mq / Iroh relay--> backend container --> SQLite
                    public DNS + TLS                                  no HTTP listener
```

Only `/api/v1` is tunneled. Static frontend assets and SPA routes stay at the gateway. Request and response bodies are streamed in 512 KiB frames, so image uploads are not constrained by `rtn-mq`'s 1 MiB per-message ceiling.

## What is authenticated

The backend creates and persists an Iroh endpoint identity, a realm authority, the one-use join grant, and the gateway membership it issued. The gateway creates its own private endpoint key on first startup and persists it separately.

The join code is a bearer enrollment credential, but it is not used to sign application traffic. Successful enrollment binds the one permitted use to the gateway's public key. Every later `rtn-mq` message is signed by that key and checked against its certificate and exact topic permissions:

- Gateway: publish `taskboard/http/request`, subscribe `taskboard/http/response`.
- Backend: subscribe `taskboard/http/request`, publish `taskboard/http/response`.

Someone who gets the code after the legitimate gateway enrolls cannot register a different key. Someone who gets it before enrollment can race the gateway and consume the only use, so deliver it as a secret. Anyone who obtains the gateway's persisted private key as well as its code can impersonate that gateway; protect the gateway volume like any other service credential.

The join code may be supplied as `TASKBOARD_RTN_JOIN_CODE`. Container environment variables are acceptable when administrators and the container runtime are trusted, but they are visible through container inspection. `TASKBOARD_RTN_JOIN_CODE_FILE` is also supported for a mounted secret. The code is read only by the native gateway process and is never compiled into frontend assets or sent to a browser.

## Repository layout and builds

The local builds expect adjacent checkouts:

```text
parent/
  rtn-mq/
  rtn-taskboard/
```

Compose passes `../rtn-mq` as a named Docker build context. To build without Compose:

```sh
docker build --build-context rtn_mq=../rtn-mq -f Dockerfile.backend -t taskboard-backend:local .
docker build --build-context rtn_mq=../rtn-mq -f Dockerfile.gateway -t taskboard-gateway:local .
```

In a registry-based deployment, build both images in CI and deploy only the appropriate image and Compose configuration to each machine.

## 1. Prepare the backend server

Create `.env` from `.env.example`. The public URL is still required on the private backend because it validates browser origins and creates invitation/Discord links:

```dotenv
TASKBOARD_BASE_URL=https://tasks.example.com
TASKBOARD_SECURE_COOKIES=true
TASKBOARD_MAX_DB_GIB=0
TASKBOARD_MAX_IMAGE_MIB=10
TASKBOARD_RTN_RELAY_ONLY=true
```

Build the image, then issue the credential while the backend service is stopped:

```sh
docker compose -f compose.backend.yaml build
docker compose -f compose.backend.yaml run --rm backend issue-gateway-code
```

Keep the printed `rtn-mq://join/...` value private. This command also initializes persistent backend identity and enrollment state in `taskboard-backend-data`. Do not run a second copy of the command against that volume while the backend is running.

Bootstrap the first editor, then start the backend:

```sh
bash -c '
  read -r -s -p "Editor password (at least 15 characters): " taskboard_password
  printf "\n" >&2
  printf "%s\n" "$taskboard_password"
' | docker compose -f compose.backend.yaml run --rm -T backend bootstrap you@company.com "Your Name"

docker compose -f compose.backend.yaml up -d
docker compose -f compose.backend.yaml ps
```

There is deliberately no `ports` entry and no HTTP listener inside the backend container. API requests can reach its in-process router only after arriving as authenticated tunnel frames. `TASKBOARD_RTN_RELAY_ONLY=true` also prevents Iroh from accepting a direct-IP data path.

## 2. Prepare the gateway host

Create its `.env` with the code printed by the backend. The public-origin settings remain backend-only; this keeps backend and Discord secrets out of the gateway container:

```dotenv
TASKBOARD_PUBLISH_ADDRESS=127.0.0.1
TASKBOARD_RTN_RELAY_ONLY=true
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

Configure the existing TLS reverse proxy to send `https://tasks.example.com` to `127.0.0.1:8080`. If the gateway container itself should be LAN-accessible, change `TASKBOARD_PUBLISH_ADDRESS`, but this has no effect on backend networking.

## Recovery and operations

- Both processes reconnect outbound through the relay. The gateway explicitly re-enrolls its same persisted identity after a backend restart; this does not consume another code use.
- A disconnected backend makes gateway readiness fail and API requests return `503` rather than serving stale data. Static assets remain available.
- A connection loss can make the outcome of an in-flight write unknown after the backend accepted its frames. `rtn-mq` prevents message forgery and duplicate frame delivery, but it does not turn Taskboard CRUD operations into a cross-process transaction. Reconcile state before manually repeating a write that ended in `503`.
- Back up the backend `/data` volume, including `taskboard.db` and `/data/rtn`. Back up the gateway `/data/rtn` identity independently.
- Never copy the gateway identity into a second running gateway. The current protocol intentionally supports one global gateway for this code and expects exactly one response recipient.
- Issuing another code does not revoke an existing gateway certificate. If the gateway private key is suspected compromised, take the deployment offline and replace the backend `rtn` authority/enrollment state and gateway identity together, then issue a new code. Preserve the SQLite database.

## Local two-container deployment

`compose.yaml` runs both roles on one Docker host while still forcing relay-only transport. Follow the README quick start. The services do not call each other over the Compose network, and the backend has no HTTP listener or published port.
