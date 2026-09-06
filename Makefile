.PHONY: start-gateway stop-gateway

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

password:
	bash -c '
	read -r -s -p "Editor password (at least 15 characters): " taskboard_password
	printf "\n" >&2
	printf "%s\n" "$taskboard_password"
	' | docker compose run --rm -T backend bootstrap you@company.com "Your Name"