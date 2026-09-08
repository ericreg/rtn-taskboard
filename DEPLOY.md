# Deploy the frontend and gateway to Render

The gateway Docker image includes the frontend. Render runs this public service,
while the backend and its database stay on AMD:

```text
https://numbers.run/dev/tasks
          | HTTPS
          v
Render: frontend + gateway
          | Iroh, direct preferred with relay fallback
          v
AMD: backend + database
```

Render pulls Docker images from a container registry. These instructions use
GitHub Container Registry (GHCR).

## Current deployment prerequisites

The gateway supports Free-plan identity persistence through a runtime secret.
Configure it before launching; `/dev/tasks` support and deployment coordination
still need attention:

- **Identity loading from a persistent secret.** Set
  `TASKBOARD_RTN_IDENTITY_HEX` to the existing gateway's 32-byte private key encoded
  as 64 hexadecimal characters. This overrides the identity file and loads the
  same key on every startup without depending on local storage. Render Free has
  no persistent disks and discards local files on restarts, redeployments, and
  sleep. The join code alone cannot restore the gateway identity. This requires
  image `render-2` or later; `render-1` does not support this setting.
- **Subpath support.** Navigation, assets, API calls, attachments, and generated
  invitation/reset links need to work beneath `/dev/tasks`. The backend currently
  rejects a `TASKBOARD_BASE_URL` containing a path; setting Vite's base alone is
  insufficient.
- **Deployment coordination.** Ensure the old and new gateway instances do not
  run concurrently with the same identity. The current backend supports one
  gateway; account for Render's overlapping deployments when using a service
  without a persistent disk.

Use the Render service's root URL for initial testing. No subpath setting is
available in the current code. Do not run two gateway instances with the same
key, including during an image update; coordinate a stop before starting the new
instance rather than relying on overlapping zero-downtime deployment.

A paid service can instead mount a persistent disk at `/data` for the existing
file-based identity. Subpath support is still required for the intended URL.

See [Render's Free-plan limits](https://render.com/docs/free),
[environment variables and secrets](https://render.com/docs/configure-environment-variables),
and [persistent disks](https://render.com/docs/disks).

## 1. Sign Docker into GHCR

Create a GitHub **personal access token (classic)** with `write:packages`, then
run:

```sh
docker login ghcr.io -u ericreg
```


Paste the token at the password prompt. Keep tokens and gateway credentials out
of the repository and Docker image.

See [GitHub's container registry authentication instructions](https://docs.github.com/en/packages/working-with-a-github-packages-registry/working-with-the-container-registry).

## 2. Build and push the gateway image

Run this from the `rtn-taskboard` repository, with the companion `rtn-mq` checkout
at `../rtn-mq`:

```sh
docker buildx build \
  --platform linux/amd64 \
  --build-context rtn_mq=../rtn-mq \
  --file Dockerfile.gateway \
  --label org.opencontainers.image.source=https://github.com/ericreg/rtn-taskboard \
  --tag ghcr.io/ericreg/taskboard-gateway:render-2 \
  --push .
```

Render requires `linux/amd64`. An ARM64 image built locally on an Apple Silicon
Mac cannot be deployed as-is. Building for AMD64 on that Mac may be slow; an
AMD64 build host or CI runner can run the same command.

Use a new tag for each subsequent release, such as `render-3`.

See [Render's image requirements](https://render.com/docs/deploying-an-image#image-requirements).

## 3. Create the Render Web Service

In the Render dashboard:

1. Select **New → Web Service → Existing Image**.
2. Enter `ghcr.io/ericreg/taskboard-gateway:render-2` as the image URL.
3. Add registry credentials if the image is private. GHCR packages start private;
   use username `ericreg` and a separate GitHub classic token with `read:packages`
   for Render's pull access.
4. Choose a service name, region, and compute plan. Use Free only after completing
   the prerequisites above. Keep a single gateway instance.
5. Leave the Docker command unchanged so the image's entrypoint runs the gateway.

See [Render's registry setup](https://render.com/docs/deploying-an-image#credentials-for-private-images).

## 4. Configure and enroll the gateway

Set these environment variables, replacing the example origin with the service's
actual Render URL:

```dotenv
TASKBOARD_GATEWAY_BIND=0.0.0.0:10000
TASKBOARD_GATEWAY_ORIGIN=https://YOUR-SERVICE.onrender.com
TASKBOARD_RTN_RELAY_ONLY=false
```

Use `/health/ready` as the Render health-check path.

Configure the stable gateway identity and its matching private
`TASKBOARD_RTN_JOIN_CODE` before launching. For a paid disk-based deployment,
mount the disk at `/data` and use:

```dotenv
TASKBOARD_RTN_IDENTITY=/data/rtn/gateway.key
```

For Free, add `TASKBOARD_RTN_IDENTITY_HEX` in Render's **Environment** settings.
Use the private key from the gateway that already enrolled, not its public
endpoint ID. A private export of the current Colima gateway is available locally
at `~/.local/state/taskboard-render/gateway-identity.hex`. Copy it to the clipboard
without printing it in the terminal:

```sh
pbcopy < "$HOME/.local/state/taskboard-render/gateway-identity.hex"
```

Paste it as the value of `TASKBOARD_RTN_IDENTITY_HEX`. Keep the matching
`TASKBOARD_RTN_JOIN_CODE`. Store these values only in Render's runtime environment
settings, never in this document or the Dockerfile. An empty or malformed
identity secret stops startup instead of creating a new identity.

If upgrading a failed `render-1` deployment, set the identity secret and select
the `render-2` image before retrying. A disposable key created by `render-1` is
ignored when the secret is configured. Stop any gateway already using the
exported identity before starting the Render service.

Stop the existing Colima gateway before moving its identity to Render. Reuse both
its private key and matching join code, or replace the enrollment on AMD using
the procedure in [DEPLOYMENT.md](DEPLOYMENT.md#replacing-a-gateway). Copying the
existing join code alone will not enroll a new key. Do not reseed or delete the
backend database.

Once the applicable prerequisites and credentials are in place, deploy the
service and verify readiness, login, API requests, and uploads.

Keep `TASKBOARD_RTN_RELAY_ONLY=false` on both hosts. Verify that Render-to-AMD
traffic actually selects a direct UDP path before cutover. Render's public
ingress exposes HTTP; its documentation does not establish whether its outbound
NAT will permit this particular Iroh connection. A successful API request alone
does not distinguish direct transport from relay fallback.

## 5. Connect numbers.run

After the service works at its Render URL and the image supports `/dev/tasks`:

1. Add `numbers.run` under the service's **Settings → Custom Domains**.
2. Enter the DNS records Render provides in Cloudflare, then verify the domain
   in Render. Render provisions HTTPS automatically.
3. Change `TASKBOARD_GATEWAY_ORIGIN` to `https://numbers.run` and redeploy. An
   origin contains no path; do not append `/dev/tasks` to this setting.
4. Configure the new subpath support and backend-generated links for
   `https://numbers.run/dev/tasks`. Enable `TASKBOARD_SECURE_COOKIES=true` on AMD
   for the public HTTPS deployment.
5. Test the app at `https://numbers.run/dev/tasks`, including a direct visit to a
   nested route and a browser refresh.

DNS points the whole hostname to Render. The application handles `/dev/tasks`.
There is no existing homepage to preserve.

See [Render's custom-domain instructions](https://render.com/docs/custom-domains).

## Later releases and Free-plan behavior

Push a new image tag, update the service's image reference, and deploy with the
gateway coordination described above. Pushing an image to GHCR does not
automatically trigger a Render deployment. Render also provides
**Manual Deploy → Deploy latest reference** to pull the currently configured
reference again.

Free services sleep after 15 minutes without inbound traffic and take about one
minute to wake. Preserve the identity through the configured secret so waking
does not create a new gateway key. These limits do not affect the database stored
on AMD.
