.PHONY: start-gateway stop-gateway start-backend stop-backend join-code seed prepare-data

# Compose cannot determine the invoking host user on its own.
export TASKBOARD_UID := $(shell id -u)
export TASKBOARD_GID := $(shell id -g)

start-gateway:
	docker compose -f compose.gateway.yaml up -d --build gateway
	docker compose -f compose.gateway.yaml logs -f gateway

stop-gateway:
	docker compose -f compose.gateway.yaml down

start-backend: prepare-data
	docker compose -f compose.backend.yaml up -d --build backend
	docker compose -f compose.backend.yaml logs -f backend

stop-backend:
	docker compose -f compose.backend.yaml down

prepare-data:
	@umask 077; mkdir -p ./data
	@test -w ./data && test -x ./data || { \
	  printf 'The data directory is not writable by your user. Migrate its ownership as described in README.md before starting the backend.\n' >&2; exit 1; }
	@chgrp "$$TASKBOARD_GID" ./data

join-code: prepare-data
	docker compose run --rm backend issue-gateway-code

seed: export TASKBOARD_EDITOR_EMAIL = $(value EMAIL)
seed: export TASKBOARD_EDITOR_NAME = $(value NAME)
seed: prepare-data
	@bash -eu -o pipefail -c '\
	taskboard_email="$${TASKBOARD_EDITOR_EMAIL:-}"; \
	taskboard_name="$${TASKBOARD_EDITOR_NAME:-}"; \
	if [[ -z "$$taskboard_email" ]]; then read -r -p "First editor email: " taskboard_email; fi; \
	if [[ -z "$$taskboard_name" ]]; then read -r -p "First editor display name: " taskboard_name; fi; \
	if [[ -z "$$taskboard_email" || -z "$$taskboard_name" ]]; then \
	  printf "Email and display name are required.\n" >&2; exit 1; \
	fi; \
	read -r -s -p "Editor password (at least 15 characters): " taskboard_password; \
	printf "\n" >&2; \
	printf "%s\n" "$$taskboard_password" | docker compose -f compose.backend.yaml run --rm -T backend seed "$$taskboard_email" "$$taskboard_name"'
