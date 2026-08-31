# MCP Vault behind an existing Nginx

This Compose file starts only MCP Vault and does not require an `.env` file.
An existing Nginx reaches the two published backend listeners through the NAS
LAN/VPN address.

External URLs are fixed for this deployment:

```text
Data plane: https://mcp-vault.cyt.cool
Admin:      https://mcp-vault.cyt.cool:8444
LAN Admin:  http://<NAS-IP>:8081
```

Before deployment, edit `compose.yaml` and replace these examples:

- `/path/on/nas/mcp-vault` with the absolute persistent directory on the NAS;
- `192.168.1.20` in `MCP_VAULT_ADMIN_ORIGINS` with the NAS address used by
  local browsers.

Then validate and start the deployment:

```bash
docker compose config --quiet
docker compose up -d
```

Configure the existing Nginx upstreams with the NAS LAN/VPN address:

```text
Data plane upstream: http://<NAS-IP>:8080
Admin upstream:      http://<NAS-IP>:8081
```

The public virtual host must proxy only MCP, WebDAV, public health,
`/.well-known/oauth-protected-resource`, the exact
`/.well-known/oauth-authorization-server` path, and `/oauth/` to the data
listener. OAuth discovery, registration, login, and token routes are public by
design; do not route unrelated well-known paths or any Admin route through this
rule. The `8444` Admin virtual
host proxies to the Admin listener and remains restricted to the intended
LAN/VPN sources. Preserve the original `Host` and WebDAV `Authorization`, set
`X-Forwarded-Proto: https`, and set the external forwarded port to `443` for
data or `8444` for Admin.
Disable caching for every OAuth route at both Nginx and any upstream CDN. When
upgrading from an image that served `/oauth/authorize` directly, purge that
legacy URL so a cached transaction page cannot survive the deployment.

Restrict published backend port 8080 at the Docker host firewall so only the
existing Nginx can reach it. MCP Vault trusts the forwarded HTTPS scheme
without authenticating the proxy peer, so direct untrusted access to this
plaintext listener would make that assertion forgeable. Keep port 8081 limited
to the intended LAN/VPN Admin clients.

The direct `http://<NAS-IP>:8081` Admin URL is an explicit trusted-LAN mode.
The login page warns that credentials and sessions are not encrypted on that
link. Keep port 8081 limited to the local network; use the HTTPS `8444` route
when the LAN is not fully trusted.

Persistent application state is stored directly in the configured NAS bind
mount. This variant deliberately does not use a Docker named volume or tmpfs.
Because `/tmp` would otherwise be unwritable, it also does not enable a
read-only container root filesystem; capability dropping, PID/memory limits,
and `no-new-privileges` remain enabled.

GUI-managed NAS directories are often owned by a host-specific account that
cannot be changed to the image's default UID/GID 999. This variant therefore
overrides the runtime user to `0:0`, drops every Linux capability, and adds
back only `DAC_OVERRIDE` so the process can write the `/data` bind mount. It
does not use privileged mode. Files created under the NAS directory will be
owned by root from the container/host namespace. If the NAS applies an ACL
that explicitly denies Docker access, grant that directory read/write access
through the NAS file-management UI; Compose cannot override a host ACL deny.
