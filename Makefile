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