.PHONY: fmt fmt-check lint test frontend-install frontend-lint frontend-test frontend-build docs-check check build docker-build image-sbom image-scan run conformance webdav-litmus e2e migration-check perf-baseline release-check

CARGO ?= cargo
PNPM ?= pnpm

fmt:
	$(CARGO) fmt --all

fmt-check:
	$(CARGO) fmt --all --check

lint:
	$(CARGO) clippy --workspace --all-targets --all-features -- -D warnings

test:
	$(CARGO) test --workspace --all-features

frontend-install:
	CI=true $(PNPM) --dir frontend/admin install --frozen-lockfile

frontend-lint: frontend-install
	CI=true $(PNPM) --dir frontend/admin lint

frontend-test: frontend-install
	CI=true $(PNPM) --dir frontend/admin test

frontend-build: frontend-install
	CI=true $(PNPM) --dir frontend/admin build

docs-check:
	bash scripts/check-docs.sh
	$(CARGO) doc --workspace --no-deps

check: fmt-check lint test frontend-lint frontend-test frontend-build docs-check

build: frontend-build
	$(CARGO) build --workspace --locked

docker-build:
	docker build --tag mcp-vault:dev .

image-sbom:
	command -v syft >/dev/null || (echo "syft is required for image-sbom" >&2; exit 1)
	syft mcp-vault:dev -o spdx-json=sbom-mcp-vault.json

image-scan:
	command -v trivy >/dev/null || (echo "trivy is required for image-scan" >&2; exit 1)
	trivy image --severity HIGH,CRITICAL --exit-code 1 mcp-vault:dev

run: frontend-build
	$(CARGO) run -p mcp-vault-server

conformance:
	bash scripts/conformance/mcp.sh

webdav-litmus:
	bash scripts/interop/webdav-litmus.sh

e2e:
	bash scripts/interop/http-smoke.sh

migration-check:
	bash scripts/release/check-migrations.sh

perf-baseline:
	bash scripts/perf/baseline.sh

release-check:
	bash scripts/release/check-release.sh
