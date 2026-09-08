# Taskboard

A self-hosted project management app built with Svelte, TypeScript, Rust, [Turso](https://github.com/tursodatabase/turso), and `rtn-mq`. It deploys as two non-root `scratch` containers:

- The public **gateway** serves the compiled frontend and forwards same-origin `/api/v1` requests.
- The private **backend** owns Turso, authorization, jobs, and the optional Discord bot. It has no HTTP listener or published port.

The containers exchange signed, acknowledged, streaming frames over Iroh, preferring direct connections with relay fallback. The browser uses ordinary HTTPS, cookies, and CSRF protection; it never receives the join code or an Iroh key. See [Split deployment](DEPLOYMENT.md) for the remote-server setup and security model.

## Breaking storage change

This release uses Turso's native embedded Rust engine (`turso` 0.8.0-pre.8). No cloud account, server, or database token is required. The old database drivers and migrations have been removed. Existing databases are rejected; there is no import or upgrade path.

For an existing installation, stop both services and preserve the complete old backend data directory separately. Start with a fresh `./data`, run `just backend`, `just seed`, and `just join-code`, then replace the gateway's join code and restart both services. Seeding creates a new editor and backend identity; previous users, content, and enrollment grants are not carried forward. Keep old backups separate from Turso backups.

## Quick start

For a local split deployment, put `rtn-taskboard` and `rtn-mq` beside each other, then run these commands from the Taskboard repository. You need [just](https://just.systems/man/en/installation.html), Bash, Docker, and a recent Docker Compose V2 with additional build-context support. Run `just` or `just --list` to see the commands in the repository’s `justfile`.

Keep both checkouts up to date. This version requires the `rtn-mq` application storage APIs (`HostStorage`, `generate_host_state`, `host_with_storage`, and identity byte conversion) and `topics_ready`. Companion changes in `../rtn-mq` must be committed and pushed in that repository separately before pulling them on a deployment host; updating Taskboard alone does not include them.

All Compose configurations use `network_mode: host`, including one-off backend commands. On Linux, containers share the host network namespace instead of using a Compose bridge and its embedded DNS. On Docker Desktop 4.34 or later, first enable **Settings → Resources → Network → Enable host networking** ([Docker documentation](https://docs.docker.com/engine/network/drivers/host/)).

The gateway binds directly to `127.0.0.1:8080` by default; port 8080 must be free on the host. There are no Docker port mappings. `TASKBOARD_PUBLISH_ADDRESS` controls the gateway's bind address, and Iroh prefers direct connections with relay fallback by default.

### 1. Create your configuration

```sh
cp .env.example .env
```

If `.env` already exists, edit it instead of overwriting it. For a local first run, keep these settings:

```dotenv
TASKBOARD_BASE_URL=http://localhost:8080
TASKBOARD_GATEWAY_ORIGIN=http://localhost:8080
TASKBOARD_SECURE_COOKIES=false
TASKBOARD_PUBLISH_ADDRESS=127.0.0.1
TASKBOARD_MAX_DB_GIB=0
TASKBOARD_MAX_IMAGE_MIB=10
```

Discord is optional. Leave its token and server ID empty unless you want to enable the bot.

`just` recipes automatically run both containers as the invoking host user's UID/GID. Before using the direct `docker compose` commands below, export those IDs in your shell on each machine:

```sh
export TASKBOARD_UID="$(id -u)" TASKBOARD_GID="$(id -g)"
```

The backend stores Turso and its transport identity in `./data` on the host. The backend recipes create this directory as your user. For a new installation using Compose directly, create it first so Docker does not create a root-owned bind-mount directory:

```sh
umask 077
mkdir -p ./data
```

For a previous release, follow [Breaking storage change](#breaking-storage-change). For an existing Turso installation whose files are owned by UID 10001, follow [Changing the container user](#changing-the-container-user).

### 2. Set up the backend

For a new installation, run these commands in order while the backend is initially stopped:

```sh
just backend
just seed
just join-code
just start-backend
```

`just backend` builds the backend image using `compose.backend.yaml`. The first build downloads dependencies and compiles Rust, so allow several minutes and a few gigabytes of free disk space.

`just seed` prompts for the first editor's email, display name, and password. The password is hidden and passed to the container over standard input. You can also supply the email and name as positional arguments:

```sh
just seed "you@company.com" "Your Name"
```

The underlying command is `taskboard seed EMAIL [NAME]`, with the password on stdin. It creates the database schema and commits the first editor, a generated backend private key, and the CBOR authority/enrollment state together. Everything is stored in `taskboard.db`; no backend `.key` or `.cbor` files are written. Seeding runs offline and refuses to overwrite an installation that already has users or a backend identity. Passwords are stored as salted Argon2id hashes. There are no default login credentials.

`just join-code` connects to the relay using the seeded identity and persists the one-use enrollment grant in Turso. Run it while the backend service is stopped. Copy the single `rtn-mq://join/...` line into `TASKBOARD_RTN_JOIN_CODE` in the gateway host's `.env` (the same `.env` for a local deployment).

`just start-backend` starts the built image and follows its logs. Press `Ctrl+C` to stop following logs; the backend keeps running. After source changes, run `just backend` and then `just start-backend` to rebuild and apply the new image.

### 3. Start the gateway

After setting its join code and browser origin in `.env`, run this on the gateway host (or in the same checkout for a local deployment):

```sh
just start-gateway
```

This builds and starts the gateway, then follows its logs. The gateway generates its own key on first launch and keeps it in `taskboard-gateway-data`, so preserve that volume across upgrades and restarts. Press `Ctrl+C` to stop following logs; the gateway keeps running.

Open **[http://localhost:8080](http://localhost:8080)** and sign in with the account you created.

Create your first project, then add tasks. Use **Workspace management → Create invitation** to generate signup links for teammates. New accounts begin as viewers; an editor can promote them. All users can access all projects.

Taskboard has two roles. Viewers can browse all projects and tasks and manage their own preferences, notifications, timezone, Discord link, and watch list. Editors can additionally create and modify content, archive and permanently delete tasks, invite and manage users, and change the storage threshold. Taskboard always preserves at least one active editor.

## Everyday commands

| Action | Command |
| --- | --- |
| Build the backend image | `just backend` |
| Build, seed, and generate a code for a fresh backend | `just init-backend` |
| Initialize a new backend and first editor | `just seed` |
| Generate a gateway enrollment code (backend stopped) | `just join-code` |
| Replace a gateway after losing its key or changing Docker runtime (backend stopped) | `just replace-gateway-code` |
| Start the backend and follow logs | `just start-backend` |
| Stop the backend | `just stop-backend` |
| Build/start the gateway and follow logs | `just start-gateway` |
| Stop the gateway | `just stop-gateway` |
| Start | `docker compose up -d` |
| Stop | `docker compose stop` |
| Restart | `docker compose restart backend gateway` |
| Follow logs | `docker compose logs -f backend gateway` |
| Check container health | `docker compose ps` |
| Check the public gateway | `docker compose exec gateway /taskboard-gateway healthcheck` |
| Report database size and threshold (backend stopped) | `docker compose -f compose.backend.yaml run --rm backend status` |
| Rebuild after source changes | `docker compose up -d --build` |
| Apply changes to `.env` | `docker compose up -d --force-recreate` |
| Remove the containers, keeping data | `docker compose down` |

The scratch images have no shell, package manager, Node runtime, or Turso CLI. Taskboard uses Turso's single-process mode; stop the backend before running `seed`, `status`, `invalidate-sessions`, or `issue-gateway-code`. Live storage status remains available in Workspace management and Discord.

## Configuration

Compose reads `.env` for interpolation and passes only the explicitly listed values to the appropriate container. Backend-only settings such as the Discord token are not placed in the gateway environment.

| Variable | Default | Purpose |
| --- | --- | --- |
| `TASKBOARD_UID`, `TASKBOARD_GID` | Current user, exported by `just` | Host UID/GID for both runtime users and image data ownership; required for direct Compose commands |
| `TASKBOARD_BASE_URL` | `http://localhost:8080` | Backend-only public URL for invitation, password-reset, and Discord links; not a browser-origin allowlist |
| `TASKBOARD_GATEWAY_ORIGIN` | `http://localhost:8080` | Gateway-only allowed browser origin, including scheme and port; independent of the backend link URL |
| `TASKBOARD_SECURE_COOKIES` | `false` in the example | Use `false` for local HTTP; set `true` when serving the app over HTTPS |
| `TASKBOARD_PUBLISH_ADDRESS` | `127.0.0.1` | Gateway HTTP bind address in host-networked Compose; set `0.0.0.0` for LAN access |
| `TASKBOARD_RTN_JOIN_CODE` | Required by gateway | One-use enrollment secret; it never enters browser assets |
| `TASKBOARD_RTN_JOIN_CODE_FILE` | Empty | Alternative file containing the join code, useful with a mounted secret |
| `TASKBOARD_RTN_RELAY_ONLY` | `false` | Prefer direct Iroh connections with automatic relay fallback; `true` explicitly forces relay-only transport |
| `TASKBOARD_MAX_DB_GIB` | `0` | Initial database content threshold in whole GiB; `0` means unlimited |
| `TASKBOARD_MAX_IMAGE_MIB` | `10` | Maximum size of one uploaded image in whole MiB; must be greater than zero |
| `TASKBOARD_DISCORD_TOKEN` | Empty | Optional Discord bot token |
| `TASKBOARD_DISCORD_GUILD_ID` | Empty | Company Discord server ID; required when a bot token is supplied |
| `TASKBOARD_DATABASE` | `/data/taskboard.db` in the image | Turso database path |
| `TASKBOARD_RTN_IDENTITY` | `/data/rtn/gateway.key` in the gateway image | Gateway-only private endpoint key; backend key and enrollment state live in Turso |
| `TASKBOARD_GATEWAY_BIND` | `127.0.0.1:8080` in Compose; `0.0.0.0:8080` in the image | Gateway HTTP listener; Compose derives this from `TASKBOARD_PUBLISH_ADDRESS` and port 8080 |
| `RUST_LOG` | See `.env.example` | Application logging filter |

Keep the path settings at their container defaults unless you also adjust the volumes. Configure the Compose gateway listener through `TASKBOARD_PUBLISH_ADDRESS`. The backend has no HTTP bind setting or HTTP listener; its only application transport is the authenticated `rtn-mq` tunnel.

### Public DNS and HTTPS

Point the website DNS name at the gateway host and terminate TLS there (or in a reverse proxy in front of port 8080). Set the gateway's browser origin and the backend's link URL/cookie policy in their respective deployments:

```dotenv
TASKBOARD_PUBLISH_ADDRESS=127.0.0.1
TASKBOARD_GATEWAY_ORIGIN=https://tasks.example.com
TASKBOARD_BASE_URL=https://tasks.example.com
TASKBOARD_SECURE_COOKIES=true
```

The DNS and TLS configuration is only for browsers reaching the gateway. The backend uses direct UDP when reachable and needs outbound relay access for discovery and fallback. It requires no public HTTP listener, DNS record, or Docker port mapping. Browser-origin validation happens at the gateway against `TASKBOARD_GATEWAY_ORIGIN`, before requests enter the tunnel. `TASKBOARD_BASE_URL` is only for generated links and does not need to match the gateway origin for login or writes to work. The backend still enforces user sessions, CSRF tokens, and permissions.

When upgrading, set `TASKBOARD_GATEWAY_ORIGIN` explicitly if you use anything other than `http://localhost:8080`, then rebuild and recreate both services using their respective Compose files. The gateway does not inherit `TASKBOARD_BASE_URL`. For example, opening `http://127.0.0.1:8080` requires `TASKBOARD_GATEWAY_ORIGIN=http://127.0.0.1:8080` on the gateway machine only. No new join code is needed.

## Storage threshold and images

Images are stored **inside Turso**, in chunks alongside their attachment metadata. They count toward the database threshold. Each image is also limited by the backend's `TASKBOARD_MAX_IMAGE_MIB` setting, which defaults to 10 MiB and is checked against the image bytes rather than multipart overhead.

To start a fresh database with a 1 GiB threshold:

```dotenv
TASKBOARD_MAX_DB_GIB=1
```

**This environment variable seeds the setting only when the database is first initialized.** After that, change the persistent threshold in **Workspace management → Storage, under your control**. The UI uses MiB; enter `0` for unlimited. Changing the environment variable alone will not replace an existing saved threshold.

- The threshold measures Turso's used pages (`page_count - freelist_count`), including images and application records. Deleted pages become reusable capacity; the database file does not automatically shrink.
- The per-image limit is container configuration. Change `TASKBOARD_MAX_IMAGE_MIB` and recreate the backend container to update it.
- Creating content or enlarging task/project text is blocked when storage is full. Content writes that would cross the threshold are rolled back.
- Viewing, signing in, changing task status, archiving, and permanently deleting tasks remain available.
- Remove images, permanently delete archived tasks, or raise the threshold to recover space. Moving a task to Archive preserves its content and does not free that space.
- The threshold is an admission limit for user content, not a hard cap on all disk use. Operational records can still grow, and Turso's write-ahead log consumes additional space.
- Uploads are temporarily spooled to the container's `/tmp` directory before being committed to Turso, so the host needs temporary disk space for them too.

View usage in Workspace management, with the `status` CLI subcommand, or with Discord `/status`.

## Discord setup

Taskboard requires a **Discord bot** for slash commands and direct-message notifications. A webhook URL will not work in these configuration fields; webhook support is not implemented.

1. Open the [Discord Developer Portal](https://discord.com/developers/applications) and create an application named **Taskboard**. Open **Bot → Reset Token** to generate and copy its bot token.
2. Under **Installation**, enable **Guild Install**. Select the `bot` and `applications.commands` scopes for Guild Install, then use the install link to add the bot to your company server. See [Discord's bot setup guide](https://docs.discord.com/developers/quick-start/getting-started). Taskboard uses slash commands rather than reading ordinary message content.
3. In Discord, enable **User Settings → Advanced → Developer Mode**. Right-click your server icon and select **Copy Server ID**. “Guild ID” means the server ID, not a channel ID or application ID. See [Discord's ID instructions](https://support.discord.com/hc/en-us/articles/206346498-Where-can-I-find-my-User-Server-Message-ID).
4. Put the bot token and server ID in your local `.env`:

   ```dotenv
   TASKBOARD_DISCORD_TOKEN=your-bot-token
   TASKBOARD_DISCORD_GUILD_ID=your-server-id
   ```

5. From the project directory, recreate the container to apply the configuration:

   ```sh
   docker compose up -d --force-recreate
   ```

6. Check the connection in **Workspace management**. You can inspect the container logs with `docker compose logs -f backend`.
7. In **Taskboard → Settings → Discord**, generate a linking code. Run the displayed `/taskboard link` command in your Discord server, then return to Taskboard to confirm your Discord identity.
8. Try `/taskboard list` in Discord to verify the connection and account link. Each teammate must link their own Taskboard account before using the commands.

Keep the real token in your local `.env`; do not commit it. The bot connects outbound to Discord, so it does not require an incoming Discord webhook endpoint.

Available commands for linked active users:

| Command | Behavior |
| --- | --- |
| `/status` | Current database size, image bytes, content threshold, and whether new content is blocked |
| `/taskboard status` | Same storage report |
| `/taskboard list` | Show task reference, title, and assignee (or “Unassigned”); supports project, assignee, scope, status, due-date, and page options |
| `/taskboard view task:TB-123` | Task summary and website link |
| `/taskboard finish task:TB-123` | Mark Done |
| `/taskboard cancel task:TB-123` | Mark Canceled while retaining the task |
| `/taskboard delete task:TB-123` | Confirm, then move to Archive |
| `/taskboard watch task:TB-123` | Subscribe to task activity |
| `/taskboard unwatch task:TB-123` | Stop watching |
| `/taskboard help` | Command help |

Notification preferences are in user Settings. Due reminders are scheduled for 9am the day before, the day of, and once after a missed deadline, in the task's recorded time zone. Watcher activity and due reminders can be sent as Discord DMs. Recipients must allow the bot to DM them; delivery failures appear in Workspace management.

`/taskboard list` defaults to everyone's active tasks, including unassigned tasks, matching the website's **All tasks → Active tasks** view. Select **scope: My tasks** to show only your assignments, or **status: All** to include Done and Canceled tasks. Results are paginated with eight tasks per page; use the `page` option to see more.

Task lists are visible only to the person running the command by default. Use `/taskboard list public:true` to share the response with everyone who can see the Discord channel. Omit `public` or set it to `false` for a private response. The person running the command still needs a linked, active Taskboard account; opening task links still requires signing in to Taskboard.

Taskboard runs normally without Discord. Keep both Discord variables empty to use only the web app and in-app notifications.

## Time zones

Database timestamps and calculated deadline instants are stored in UTC. Each account starts with `UTC` and can select an IANA local time zone, such as `America/Los_Angeles`, under **Settings → Your profile**. Taskboard converts activity timestamps for display and uses the user's selected zone when that user creates or changes a due date. The recorded zone stays with the task so its calendar deadline and 9am reminders remain stable across daylight-saving changes.

## Data and manual backups

Both backend Compose configurations bind-mount **`./data:/data`**. The Turso database is `./data/taskboard.db` on the host, alongside its Turso sidecar files. The `rtn_identity` table contains the backend private key and CBOR authority, grants, and redeemed membership state as BLOBs. Seed and startup keep the database and its sidecars private (`0600` on Unix). This directory is ignored by Git. The separate **`taskboard-gateway-data`** named volume still holds the enrolled gateway identity.

### Changing the container user

Both services use the host UID/GID supplied by `just` (or exported for direct Compose). The image builds also use those IDs so a newly created gateway volume is writable by that user. This uses Docker's [runtime user setting](https://docs.docker.com/reference/compose-file/services/#user); standalone builds without UID/GID build arguments retain the non-root `10001:10001` fallback. Do not run `just` with `sudo` unless you intentionally want root IDs.

Changing the runtime user does not change existing file ownership. When upgrading from UID 10001, stop the backend on its host and transfer ownership of the existing directory, without deleting or reinitializing it:

```sh
export TASKBOARD_UID="$(id -u)" TASKBOARD_GID="$(id -g)"
docker compose -f compose.backend.yaml stop backend
sudo chown -R "$TASKBOARD_UID:$TASKBOARD_GID" ./data
sudo chmod 0700 ./data
docker compose -f compose.backend.yaml up -d --build backend
```

The gateway's existing named volume needs the same one-time ownership transfer on its own host. Stop the gateway and inspect its `/data` mount first. For the default project name, the volume is `rtn-taskboard_taskboard-gateway-data`; substitute the inspected name if yours differs:

```sh
export TASKBOARD_UID="$(id -u)" TASKBOARD_GID="$(id -g)"
docker compose -f compose.gateway.yaml stop gateway
docker inspect rtn-taskboard-gateway-1 --format '{{json .Mounts}}'
docker volume inspect rtn-taskboard_taskboard-gateway-data
docker run --rm --network none --user 0:0 \
  --mount type=volume,src=rtn-taskboard_taskboard-gateway-data,dst=/data \
  alpine:3 chown -R "$TASKBOARD_UID:$TASKBOARD_GID" /data
docker compose -f compose.gateway.yaml up -d --build gateway
```

Only the one-off ownership helper runs as root; the application services run as your user. Keep backend data private and the gateway’s `rtn` directory at `0700` with private key files. No new join code or database reset is needed.

Rebuilding images or using `docker compose down` preserves this data. `docker compose down -v` does not delete the bind-mounted host directory, but **does delete the gateway's named volume and enrolled identity**.

### Manual backups

Backup scheduling and retention are yours to manage. For a simple consistent manual copy, stop the app, copy the complete data directory into a new timestamped destination, then start it again:

```sh
taskboard_backup_dir="backups/$(date +%Y%m%d-%H%M%S)"
mkdir -p "$taskboard_backup_dir"
docker compose stop backend
docker compose cp backend:/data "$taskboard_backup_dir/"
docker compose start backend
```

The copy is under `$taskboard_backup_dir/data/`. It contains credentials and company content, so keep it private. Do not copy only the main database file while the application is writing to it.

After restoring an older backup, reconcile account access and content changes since that backup. You can invalidate saved sessions and outstanding invitation/reset/link tokens with:

```sh
docker compose -f compose.backend.yaml stop backend
docker compose -f compose.backend.yaml run --rm backend invalidate-sessions
docker compose -f compose.backend.yaml start backend
```

## Troubleshooting

- **Build reports missing `rtn_mq::HostStorage`, `generate_host_state`, or `topics_ready`:** the adjacent `../rtn-mq` checkout lacks the companion API changes. Publish those changes from the development checkout, then run `git -C ../rtn-mq pull --ff-only` on the build host and retry `just backend`. Docker copies that checkout through the `rtn_mq` build context; clearing its cache cannot add missing source changes.
- **Docker reports `metadata_v2.db: read-only file system`:** free at least several GiB on the host, restart Docker Desktop, then run `docker builder prune -f` to remove unused build cache and retry `docker compose build`. Build-cache pruning does not remove either Taskboard data volume. Do not use Docker Desktop's **Clean / Purge data** option if a volume contains data you need.
- **Unsupported database:** this is a forward-only storage change. Preserve the old data directory separately and seed a fresh installation; startup never imports old records.
- **Database is locked:** stop the running backend before using a database CLI command. Run only one backend process per data directory.
- **Seed reports an initialized database:** users or a backend identity already exist. Seed never resets an account or rotates an existing identity. Sign in to the existing account and use Workspace management for invitations or password reset links.
- **The backend says it is not seeded:** run `just seed` against the same database before first startup. Previous releases require a fresh data directory as described in [Breaking storage change](#breaking-storage-change).
- **Sign-in or editing fails with a permission error:** if the error says the gateway rejected the browser origin, set `TASKBOARD_GATEWAY_ORIGIN` on the gateway machine to the URL you visit (including scheme and port), then recreate the gateway. Changing the backend's `TASKBOARD_BASE_URL` does not change this policy. For HTTP, secure cookies must be disabled on the backend.
- **The threshold does not change after editing `.env`:** change it in Workspace management; the environment value initializes new databases only.
- **Port 8080 is already in use:** stop the conflicting service or change the gateway listener port in Compose and update `TASKBOARD_GATEWAY_ORIGIN` to match the browser URL. Update `TASKBOARD_BASE_URL` too if generated links should use that URL. Host networking has no separate published/container ports.
- **Discord stays disconnected:** confirm the bot token, server ID, installation, and outbound connectivity; inspect `docker compose logs backend`.
- **Gateway repeatedly reconnects or API requests return `503`:** see [Diagnosing disconnects](DEPLOYMENT.md#diagnosing-disconnects). Enable the `rtn_mq=info,iroh=warn,iroh_relay=warn` filters on both hosts so the underlying close/rejection reason is captured. New requests wait up to eight seconds for both tunnel subscriptions; accepted requests are not blindly replayed.
- **Join reports `queue or memory budget exhausted` after switching to Colima or losing the gateway volume:** this also means a one-use join code was consumed by another gateway key, or the backend's single gateway membership slot is occupied. Docker Desktop and Colima keep separate named volumes. Restore the original gateway key, or follow [Replacing a gateway](DEPLOYMENT.md#replacing-a-gateway) to invalidate the old enrollment and issue a replacement without resetting application data. Adding VM memory or changing bridge/NAT settings cannot free an enrollment slot.
- **An image cannot be uploaded:** PNG, JPEG, GIF, and WebP are supported. Check `TASKBOARD_MAX_IMAGE_MIB`, the database threshold, and host free space.

## Development checks and current verification

The Rust checks cover Turso-backed backend workflows, atomic seeding, rejection of previous databases, enrollment persistence across restarts, and a 10 MiB image upload/download through `rtn-mq`. The workflow suite also checks that 10 MiB plus one byte is rejected without persistence. Live Discord verification remains environment-specific.

To rerun the existing checks locally, install Rust and Node.js, then run:

```sh
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
npm --prefix frontend ci
npm --prefix frontend run build
```

For task-panel browser checks, run `npx playwright install chromium --only-shell` inside `frontend` once, then run `npm --prefix frontend run test:e2e` from the repository root. The suite starts a temporary frontend on port 5174 and uses an isolated test workspace; it does not modify backend data.

The original design is in [TASKBOARD_PLAN.md](TASKBOARD_PLAN.md). This README describes the current container setup and supersedes the plan's earlier proposals for filesystem image storage and automatic backup retention.
