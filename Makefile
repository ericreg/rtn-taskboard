.PHONY: start-gateway stop-gateway start-backend stop-backend join-code password

start-gateway:
	docker compose -f compose.gateway.yaml up -d --build gateway
	docker compose -f compose.gateway.yaml logs -f gateway

stop-gateway:
	docker compose -f compose.gateway.yaml down

start-backend:
	docker compose -f compose.backend.yaml up -d --build backend
	docker compose -f compose.backend.yaml logs -f backend

stop-backend:
	docker compose -f compose.backend.yaml down

join-code:
	docker compose run --rm backend issue-gateway-code

password: export TASKBOARD_EDITOR_EMAIL = $(value EMAIL)
password: export TASKBOARD_EDITOR_NAME = $(value NAME)
password:
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
	printf "%s\n" "$$taskboard_password" | docker compose -f compose.backend.yaml run --rm -T backend bootstrap "$$taskboard_email" "$$taskboard_name"'
