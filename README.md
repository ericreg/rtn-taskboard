# Taskboard

A self-hosted project management app built with Svelte, TypeScript, Rust, and SQLite. The Rust backend serves the compiled frontend and runs the optional Discord bot in **one container**. The production image uses `FROM scratch` and runs as a non-root user.

## Quick start

You need Docker running and a recent Docker Compose V2 installation. Docker Desktop includes both. Run these commands from the repository root.

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
```

Discord is optional. Leave its token and server ID empty unless you want to enable the bot.

### 2. Build the image

```sh
docker compose build
```

The first build downloads dependencies and compiles Rust, so allow several minutes and a few gigabytes of free disk space. Subsequent builds reuse caches. Rebuild from the current source even if a `taskboard:local` image already exists from development.

### 3. Create the first editor

Replace the email and display name below. This Bash command prompts for the password without echoing it or putting it in your command history, then passes it to the container over standard input:

```sh
bash -c '
  read -r -s -p "Editor password (at least 15 characters): " taskboard_password
  printf "\n" >&2
  printf "%s\n" "$taskboard_password"
' | docker compose run --rm -T taskboard bootstrap you@company.com "Your Name"
```

The command creates the database and first editor, then exits. Passwords are stored as salted Argon2id hashes. There are no default login credentials.

Run bootstrap only once. If an editor already exists, use **Workspace management** in the app to invite additional users and change their roles.

### 4. Start Taskboard

```sh
docker compose up -d
docker compose ps
docker compose logs -f taskboard
```

Open **[http://localhost:8080](http://localhost:8080)** and sign in with the account you created. Press `Ctrl+C` to stop following the logs; the container keeps running.

Create your first project, then add tasks. Use **Workspace management → Create invitation** to generate signup links for teammates. New accounts begin as viewers; an editor can promote them. All users can access all projects.

Taskboard has two roles. Viewers can browse all projects and tasks and manage their own preferences, notifications, timezone, Discord link, and watch list. Editors can additionally create and modify content, archive and permanently delete tasks, invite and manage users, and change the storage threshold. Taskboard always preserves at least one active editor.

## Everyday commands

| Action | Command |
| --- | --- |
| Start | `docker compose up -d` |
| Stop | `docker compose stop` |
| Restart | `docker compose restart taskboard` |
| Follow logs | `docker compose logs -f taskboard` |
| Check container health | `docker compose ps` |
| Run the application health check | `docker compose exec taskboard /taskboard healthcheck` |
| Report database size and threshold | `docker compose exec taskboard /taskboard status` |
| Rebuild after source changes | `docker compose up -d --build` |
| Apply changes to `.env` | `docker compose up -d --force-recreate` |
| Remove the container, keeping its data | `docker compose down` |

The scratch image has no shell, package manager, Node runtime, or SQLite command-line tool. Run `/taskboard` subcommands directly with `docker compose exec`; `sh` and `bash` are not available inside it.

## Configuration

Compose reads `.env` and supplies it to the container.

| Variable | Default | Purpose |
| --- | --- | --- |
| `TASKBOARD_BASE_URL` | `http://localhost:8080` | Exact browser origin, including scheme and port; also used in invitation and Discord links |
| `TASKBOARD_SECURE_COOKIES` | `false` in the example | Use `false` for local HTTP; set `true` when serving the app over HTTPS |
| `TASKBOARD_PUBLISH_ADDRESS` | `127.0.0.1` | Host interface on which Compose publishes port 8080 |
| `TASKBOARD_MAX_DB_GIB` | `0` | Initial database content threshold in whole GiB; `0` means unlimited |
| `TASKBOARD_DISCORD_TOKEN` | Empty | Optional Discord bot token |
| `TASKBOARD_DISCORD_GUILD_ID` | Empty | Company Discord server ID; required when a bot token is supplied |
| `TASKBOARD_DATABASE` | `/data/taskboard.db` in the image | SQLite database path |
| `TASKBOARD_FRONTEND` | `/www` in the image | Compiled frontend directory |
| `TASKBOARD_BIND` | `0.0.0.0:8080` | HTTP listener inside the container |
| `RUST_LOG` | `taskboard=info,tower_http=info` | Application logging filter |

Keep the last three path/listener settings at their container defaults unless you also adjust the volume or port configuration.

### Access from another machine

For example, if your host's LAN address is `192.168.1.50`, set:

```dotenv
TASKBOARD_PUBLISH_ADDRESS=0.0.0.0
TASKBOARD_BASE_URL=http://192.168.1.50:8080
TASKBOARD_SECURE_COOKIES=false
```

Then recreate the container and open that exact address:

```sh
docker compose up -d --force-recreate
```

If you provide HTTPS through your own reverse proxy, set the base URL to your HTTPS origin and enable secure cookies. Taskboard itself listens for HTTP inside the container.

The browser URL must match `TASKBOARD_BASE_URL`. For example, using `127.0.0.1` in the browser while the configured origin is `localhost` can cause write requests to be rejected by origin protection.

## Storage threshold and images

Images are stored **inside SQLite**, in chunks alongside their attachment metadata. They count toward the database threshold. There is no separate application-level image-size cap; uploads remain subject to the configured database threshold, available disk space, and underlying system limits.

To start a fresh database with a 1 GiB threshold:

```dotenv
TASKBOARD_MAX_DB_GIB=1
```

**This environment variable seeds the setting only when the database is first initialized.** After that, change the persistent threshold in **Workspace management → Storage, under your control**. The UI uses MiB; enter `0` for unlimited. Changing the environment variable alone will not replace an existing saved threshold.

- The threshold measures SQLite's allocated pages, including images and application records.
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

6. Check the connection in **Workspace management**. You can inspect the container logs with `docker compose logs -f taskboard`.
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

The Compose named volume **`taskboard-data`** holds `/data/taskboard.db` and any SQLite sidecar files. Compose prefixes the actual volume name with the project name. Projects, tasks, accounts, images, and saved settings all live in that database.

Rebuilding the image or using `docker compose down` preserves the named volume. **`docker compose down -v` deletes it and its application data.**

Backup scheduling and retention are yours to manage. For a simple consistent manual copy, stop the app, copy the complete data directory into a new timestamped destination, then start it again:

```sh
taskboard_backup_dir="backups/$(date +%Y%m%d-%H%M%S)"
mkdir -p "$taskboard_backup_dir"
docker compose stop taskboard
docker compose cp taskboard:/data "$taskboard_backup_dir/"
docker compose start taskboard
```

The copy is under `$taskboard_backup_dir/data/`. It contains credentials and company content, so keep it private. Do not copy only the main database file while the application is writing to it.

After restoring an older backup, reconcile account access and content changes since that backup. You can invalidate saved sessions and outstanding invitation/reset/link tokens with:

```sh
docker compose exec taskboard /taskboard invalidate-sessions
```

## Troubleshooting

- **Docker reports `metadata_v2.db: read-only file system`:** free at least several GiB on the host, restart Docker Desktop, then run `docker builder prune -f` to remove unused build cache and retry `docker compose build`. Build-cache pruning does not remove the `taskboard-data` volume. Do not use Docker Desktop's **Clean / Purge data** option if the volume contains data you need.
- **First-editor setup reports an existing editor:** bootstrap has already run against this volume. Sign in to that account and use Workspace management for invitations or password reset links.
- **The app says it needs setup:** run the bootstrap command against the same Compose project and volume as the server.
- **Sign-in or editing fails with a permission error:** check that your browser's origin matches `TASKBOARD_BASE_URL`. For HTTP, secure cookies must be disabled.
- **The threshold does not change after editing `.env`:** change it in Workspace management; the environment value initializes new databases only.
- **Port 8080 is already in use:** stop the conflicting service or change the host port in `compose.yaml` and update `TASKBOARD_BASE_URL` to match. Keep the container port at 8080.
- **Discord stays disconnected:** confirm the bot token, server ID, installation, and outbound connectivity; inspect `docker compose logs taskboard`.
- **An image cannot be uploaded:** PNG, JPEG, GIF, and WebP are supported. Check the database threshold and host free space.

## Development checks and current verification

The frontend build, the configuration unit test, and ten automated backend workflow tests passed during implementation. The scratch image was also built successfully. Interactive browser checks, live Discord verification, and a running-container smoke test were not completed before development was stopped.

To rerun the existing checks locally, install Rust and Node.js, then run:

```sh
cargo test --workspace
npm --prefix frontend ci
npm --prefix frontend run build
```

The original design is in [TASKBOARD_PLAN.md](TASKBOARD_PLAN.md). This README describes the current container setup and supersedes the plan's earlier proposals for per-image limits, filesystem image storage, and automatic backup retention.
