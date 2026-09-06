# Compose cannot determine the invoking host user on its own.
export TASKBOARD_UID := `id -u`
export TASKBOARD_GID := `id -g`

# List available commands.
default:
    @just --list

# Build the backend image.
backend:
    docker compose -f compose.backend.yaml build backend

# Build and start the gateway, then follow its logs.
start-gateway:
    docker compose -f compose.gateway.yaml up -d --build gateway
    docker compose -f compose.gateway.yaml logs -f gateway

# Stop and remove the gateway container, preserving its data volume.
stop-gateway:
    docker compose -f compose.gateway.yaml down

# Start the built backend image, then follow its logs.
start-backend: prepare-data
    docker compose -f compose.backend.yaml up -d backend
    docker compose -f compose.backend.yaml logs -f backend

# Stop and remove the backend container, preserving its data directory.
stop-backend:
    docker compose -f compose.backend.yaml down

# Create the backend data directory with private permissions.
prepare-data:
    @mkdir -p -m 700 ./data

# Generate a gateway enrollment code while the backend is stopped.
join-code: prepare-data
    docker compose -f compose.backend.yaml run --rm backend issue-gateway-code

# Seed a new database; prompt for missing email/name and always prompt for the password.
seed $email="" $name="": prepare-data
    #!/usr/bin/env bash
    set -euo pipefail
    [[ -n "$email" ]] || read -r -p "First editor email: " email
    [[ -n "$name" ]] || read -r -p "First editor display name: " name
    read -r -s -p "Editor password (at least 15 characters): " password
    printf '\n' >&2
    printf '%s\n' "$password" | docker compose -f compose.backend.yaml run --rm -T backend seed "$email" "$name"
