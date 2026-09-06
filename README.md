# Taskboard

A self-hosted project management app built with Svelte, TypeScript, Rust, SQLite, and `rtn-mq`. It deploys as two non-root `scratch` containers:

- The public **gateway** serves the compiled frontend and forwards same-origin `/api/v1` requests.
- The private **backend** owns SQLite, authorization, jobs, and the optional Discord bot. It has no HTTP listener or published port.

The containers exchange signed, acknowledged, streaming frames over an Iroh relay. The browser uses ordinary HTTPS, cookies, and CSRF protection; it never receives the join code or an Iroh key. See [Split deployment](DEPLOYMENT.md) for the remote-server setup and security model.

## Quick start

For a local split deployment, put `rtn-taskboard` and `rtn-mq` beside each other, then run these commands from the Taskboard repository. You need Docker and a recent Docker Compose V2 with additional build-context support.

All Compose configurations use `network_mode: host`, including one-off backend commands. On Linux, containers share the host network namespace instead of using a Compose bridge and its embedded DNS. On Docker Desktop 4.34 or later, first enable **Settings → Resources → Network → Enable host networking** ([Docker documentation](https://docs.docker.com/engine/network/drivers/host/)).

The gateway binds directly to `127.0.0.1:8080` by default; port 8080 must be free on the host. There are no Docker port mappings. `TASKBOARD_PUBLISH_ADDRESS` controls the gateway's bind address, and relay-only transport remains enabled by default.

### 1. Create your configuration

```sh
cp .env.example .env
```

If `.env` already exists, edit it instead of overwriting it. For a local first run, keep these settings:

```dotenv
TASKBOARD_BASE_URL=http://localhost:8080
TASKBOARD_SECURE_COOKIES=false
TASKBOARD_PUBLISH_ADDRESS=127.0.0.1
TASKBOARD_MAX_DB_GIB=0
TASKBOARD_MAX_IMAGE_MIB=10
```

Discord is optional. Leave its token and server ID empty unless you want to enable the bot.

### 2. Build the image

```sh
docker compose build
```

The first build downloads dependencies and compiles Rust, so allow several minutes and a few gigabytes of free disk space.

### 3. Issue the gateway credential

Run this while the backend service is stopped:

```sh
docker compose run --rm backend issue-gateway-code
```

Copy the single `rtn-mq://join/...` line into `TASKBOARD_RTN_JOIN_CODE` in `.env`. The code admits one distinct gateway identity. The gateway generates that key on first launch and keeps it in `taskboard-gateway-data`, so preserve that volume across upgrades and restarts.

### 4. Create the first editor

Replace the email and display name below. This Bash command prompts for the password without echoing it or putting it in your command history, then passes it to the container over standard input:

```sh
bash -c '
  read -r -s -p "Editor password (at least 15 characters): " taskboard_password
  printf "\n" >&2
  printf "%s\n" "$taskboard_password"
' | docker compose run --rm -T backend bootstrap you@company.com "Your Name"
```

The command creates the database and first editor, then exits. Passwords are stored as salted Argon2id hashes. There are no default login credentials.

Run bootstrap only once. If an editor already exists, use **Workspace management** in the app to invite additional users and change their roles.

### 5. Start Taskboard

```sh
docker compose up -d
docker compose ps
docker compose logs -f backend gateway
```

Open **[http://localhost:8080](http://localhost:8080)** and sign in with the account you created. Press `Ctrl+C` to stop following the logs; the container keeps running.

Create your first project, then add tasks. Use **Workspace management → Create invitation** to generate signup links for teammates. New accounts begin as viewers; an editor can promote them. All users can access all projects.

Taskboard has two roles. Viewers can browse all projects and tasks and manage their own preferences, notifications, timezone, Discord link, and watch list. Editors can additionally create and modify content, archive and permanently delete tasks, invite and manage users, and change the storage threshold. Taskboard always preserves at least one active editor.

## Everyday commands

| Action | Command |
| --- | --- |
| Start | `docker compose up -d` |
| Stop | `docker compose stop` |
| Restart | `docker compose restart backend gateway` |
| Follow logs | `docker compose logs -f backend gateway` |
| Check container health | `docker compose ps` |
| Check the public gateway | `docker compose exec gateway /taskboard-gateway healthcheck` |
| Report database size and threshold | `docker compose exec backend /taskboard status` |
| Rebuild after source changes | `docker compose up -d --build` |
| Apply changes to `.env` | `docker compose up -d --force-recreate` |
| Remove the containers, keeping data | `docker compose down` |

The scratch images have no shell, package manager, Node runtime, or SQLite CLI. Run their binaries directly with `docker compose exec`.

## Configuration

Compose reads `.env` for interpolation and passes only the explicitly listed values to the appropriate container. Backend-only settings such as the Discord token are not placed in the gateway environment.

| Variable | Default | Purpose |
| --- | --- | --- |
| `TASKBOARD_BASE_URL` | `http://localhost:8080` | Exact browser origin, including scheme and port; also used in invitation and Discord links |
| `TASKBOARD_SECURE_COOKIES` | `false` in the example | Use `false` for local HTTP; set `true` when serving the app over HTTPS |
| `TASKBOARD_PUBLISH_ADDRESS` | `127.0.0.1` | Gateway HTTP bind address in host-networked Compose; set `0.0.0.0` for LAN access |
| `TASKBOARD_RTN_JOIN_CODE` | Required by gateway | One-use enrollment secret; it never enters browser assets |
| `TASKBOARD_RTN_JOIN_CODE_FILE` | Empty | Alternative file containing the join code, useful with a mounted secret |
| `TASKBOARD_RTN_RELAY_ONLY` | `true` | Disables all direct-IP Iroh transport; use the configured/default relay only |
| `TASKBOARD_MAX_DB_GIB` | `0` | Initial database content threshold in whole GiB; `0` means unlimited |
| `TASKBOARD_MAX_IMAGE_MIB` | `10` | Maximum size of one uploaded image in whole MiB; must be greater than zero |
| `TASKBOARD_DISCORD_TOKEN` | Empty | Optional Discord bot token |
| `TASKBOARD_DISCORD_GUILD_ID` | Empty | Company Discord server ID; required when a bot token is supplied |
| `TASKBOARD_DATABASE` | `/data/taskboard.db` in the image | SQLite database path |
| `TASKBOARD_RTN_IDENTITY` | `/data/rtn/...key` | Persistent private endpoint key in each container |
| `TASKBOARD_RTN_STATE` | `/data/rtn/backend.cbor` | Backend authority, grants, and redeemed membership state |
| `TASKBOARD_GATEWAY_BIND` | `127.0.0.1:8080` in Compose; `0.0.0.0:8080` in the image | Gateway HTTP listener; Compose derives this from `TASKBOARD_PUBLISH_ADDRESS` and port 8080 |
| `RUST_LOG` | See `.env.example` | Application logging filter |

Keep the path settings at their container defaults unless you also adjust the volumes. Configure the Compose gateway listener through `TASKBOARD_PUBLISH_ADDRESS`. The backend has no HTTP bind setting or HTTP listener; its only application transport is the authenticated `rtn-mq` tunnel.

### Public DNS and HTTPS

Point the website DNS name at the gateway host and terminate TLS there (or in a reverse proxy in front of port 8080). Configure both deployments with the exact public origin:

```dotenv
TASKBOARD_PUBLISH_ADDRESS=127.0.0.1
TASKBOARD_BASE_URL=https://tasks.example.com
TASKBOARD_SECURE_COOKIES=true
```

The DNS and TLS configuration is only for browsers reaching the gateway. The backend needs outbound relay access but no public IP, DNS record, inbound port, Docker port mapping, or firewall rule. The browser URL must exactly match `TASKBOARD_BASE_URL`, or origin-protected write requests will be rejected.

## Storage threshold and images

Images are stored **inside SQLite**, in chunks alongside their attachment metadata. They count toward the database threshold. Each image is also limited by the backend's `TASKBOARD_MAX_IMAGE_MIB` setting, which defaults to 10 MiB and is checked against the image bytes rather than multipart overhead.

To start a fresh database with a 1 GiB threshold:

```dotenv
TASKBOARD_MAX_DB_GIB=1
```

**This environment variable seeds the setting only when the database is first initialized.** After that, change the persistent threshold in **Workspace management → Storage, under your control**. The UI uses MiB; enter `0` for unlimited. Changing the environment variable alone will not replace an existing saved threshold.

- The threshold measures SQLite's allocated pages, including images and application records.
- The per-image limit is container configuration. Change `TASKBOARD_MAX_IMAGE_MIB` and recreate the backend container to update it.
- Creating content or enlarging task/project text is blocked when storage is full. Content writes that would cross the threshold are rolled back.
- Viewing, signing in, changing task status, archiving, and permanently deleting tasks remain available.
- Remove images, permanently delete archived tasks, or raise the threshold to recover space. Moving a task to Archive preserves its content and does not free that space.
- The threshold is an admission limit for user content, not a hard cap on all disk use. Operational records can still grow, and SQLite's write-ahead log consumes additional space.
- Uploads are temporarily spooled to the container's `/tmp` directory before being committed to SQLite, so the host needs temporary disk space for them too.

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

The Compose named volume **`taskboard-backend-data`** holds `/data/taskboard.db`, its SQLite sidecar files, and `/data/rtn` backend identity/enrollment state. The separate **`taskboard-gateway-data`** volume holds the enrolled gateway identity. Compose prefixes actual volume names with the project name. This split deployment is a forward-only change and does not automatically adopt the old combined-container volume.

Rebuilding the image or using `docker compose down` preserves the named volume. **`docker compose down -v` deletes it and its application data.**

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
docker compose exec backend /taskboard invalidate-sessions
```

## Troubleshooting

- **Docker reports `metadata_v2.db: read-only file system`:** free at least several GiB on the host, restart Docker Desktop, then run `docker builder prune -f` to remove unused build cache and retry `docker compose build`. Build-cache pruning does not remove either Taskboard data volume. Do not use Docker Desktop's **Clean / Purge data** option if a volume contains data you need.
- **First-editor setup reports an existing editor:** bootstrap has already run against this volume. Sign in to that account and use Workspace management for invitations or password reset links.
- **The app says it needs setup:** run the bootstrap command against the same Compose project and volume as the server.
- **Sign-in or editing fails with a permission error:** check that your browser's origin matches `TASKBOARD_BASE_URL`. For HTTP, secure cookies must be disabled.
- **The threshold does not change after editing `.env`:** change it in Workspace management; the environment value initializes new databases only.
- **Port 8080 is already in use:** stop the conflicting service or change the host port in `compose.yaml` and update `TASKBOARD_BASE_URL` to match. Keep the container port at 8080.
- **Discord stays disconnected:** confirm the bot token, server ID, installation, and outbound connectivity; inspect `docker compose logs backend`.
- **An image cannot be uploaded:** PNG, JPEG, GIF, and WebP are supported. Check `TASKBOARD_MAX_IMAGE_MIB`, the database threshold, and host free space.

## Development checks and current verification

The frontend type/build check, all backend workflows, the tunnel protocol tests, and an end-to-end 10 MiB image upload/download through `rtn-mq` pass. The backend workflow suite also verifies that 10 MiB plus one byte is rejected without persistence. Both static `scratch` images build successfully. A running-container smoke test also verified relay-only API traffic, no backend HTTP listener, backend restart/rejoin, and gateway restart with its persisted one-use identity. Live Discord verification remains environment-specific.

To rerun the existing checks locally, install Rust and Node.js, then run:

```sh
cargo test --workspace --all-targets
cargo clippy -p taskboard-wire -p taskboard-gateway --lib --bins -- -D warnings
npm --prefix frontend ci
npm --prefix frontend run build
```

The original design is in [TASKBOARD_PLAN.md](TASKBOARD_PLAN.md). This README describes the current container setup and supersedes the plan's earlier proposals for filesystem image storage and automatic backup retention.
