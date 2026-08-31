# Optional MCP Vault + Nginx HTTPS example

MCP Vault does not require Nginx. This optional bundle demonstrates one way to
run the `linux/amd64` image behind a pinned Docker Official Nginx image. The
operator owns HTTPS, publication, and source-network policy. MCP Vault still
owns Admin username/password, session, CSRF, and Origin authentication.

Unpack the deployment archive beside the image archive, then enter its
directory:

```bash
tar -xzf mcp-vault-nginx-https-0.1.12.tar.gz
cd nginx-https
```

## 1. Load the image

Load the image archive from the parent directory:

```bash
docker load --input ../mcp-vault-0.1.12-linux-amd64.tar
docker image inspect mcp-vault:0.1.12 --format '{{.Os}}/{{.Architecture}} {{.Id}}'
```

The result must report `linux/amd64`.

## 2. Prepare configuration and writable data

```bash
cp .env.example .env
mkdir -p data certs
sudo chown -R 999:999 data
```

MCP Vault creates `data/secrets/master-key` automatically. It does not inspect
or modify the file's filesystem permission bits.

Edit `.env` and set:

- `MCP_VAULT_PUBLIC_HOST` to the public data-plane DNS name;
- `MCP_VAULT_ADMIN_BIND_IP` to an address actually configured on the Docker
  host's LAN interface;
- `MCP_VAULT_ADMIN_HOST` to the DNS name used by LAN browsers;
- `MCP_VAULT_ADMIN_SOURCE_CIDR` to the client LAN/VPN subnet allowed to open
  Admin.

The TLS certificate must cover both hostnames; they may be the same name. If
the private Docker subnet conflicts with another network, change the subnet
and both fixed container addresses together.

Copy an existing certificate and private key to:

```text
certs/fullchain.pem
certs/privkey.pem
```

The pinned Alpine Nginx container runs as numeric UID/GID `101:101`. Make the
certificate readable only by that identity:

```bash
sudo chown 101:101 certs/fullchain.pem certs/privkey.pem
chmod 0400 certs/fullchain.pem certs/privkey.pem
```

For a temporary self-signed deployment test only:

```bash
openssl req -x509 -newkey rsa:3072 -sha256 -nodes -days 30 \
  -keyout certs/privkey.pem \
  -out certs/fullchain.pem \
  -subj "/CN=vault.example.com" \
  -addext "subjectAltName=DNS:vault.example.com"
sudo chown 101:101 certs/fullchain.pem certs/privkey.pem
chmod 0400 certs/fullchain.pem certs/privkey.pem
```

Use the same hostname in `.env`. Browsers, MCP clients, and WebDAV clients
must explicitly trust the test certificate; use a publicly trusted
certificate for real remote access.

## 3. Validate and start

```bash
docker compose --env-file .env config --quiet
docker compose --env-file .env up -d
docker compose --env-file .env ps
docker compose --env-file .env logs --tail=100 mcp-vault nginx
```

Check the public endpoint:

```bash
curl --fail https://vault.example.com/health/live
curl --fail https://vault.example.com/health/ready
```

When OAuth is configured, also verify the exact metadata URL copied from the
Admin MCP page. It is public by design and must return JSON without a bearer
token:

```bash
curl --fail \
  https://vault.example.com/.well-known/oauth-protected-resource/mcp/v1/vaults/default
curl --fail \
  https://vault.example.com/.well-known/oauth-authorization-server
```

Create the independent Vault OAuth login in Admin, then add the displayed MCP
endpoint in ChatGPT. The public Nginx virtual host already forwards the narrow
`/oauth/` authorization routes; no external identity provider is required.

The public host must return `404` for `/api/v1/system`; its virtual host does
not proxy Admin paths or Admin frontend assets.

## 4. Open Admin safely

From a client inside `MCP_VAULT_ADMIN_SOURCE_CIDR`, open:

```text
https://<MCP_VAULT_ADMIN_HOST>:<MCP_VAULT_ADMIN_HTTPS_PORT>/
```

Enter the desired Admin username and password to finish setup; no container
command or setup-token copy is required. Because first setup is a first-claim
operation, every client admitted by the Admin Nginx source policy can attempt
to become the owner until one account commits. Keep that policy narrow and
complete initialization before broadening LAN/VPN access.
Admin is not reachable through the public port 443 virtual host. Do not bind
the Admin HTTPS port to `0.0.0.0`; keep the host firewall restricted to the
same LAN/VPN CIDR configured in Nginx.

After setup, separately back up `data/secrets/master-key`. Losing it
does not destroy Markdown content, but encrypted provider secrets cannot be
recovered and existing PAT digests cannot be reproduced.

## 5. Endpoints and shutdown

The Admin UI generates the final Vault-scoped connection URLs and one-time
credentials. Their shape is:

```text
https://vault.example.com/mcp/v1/vaults/<vault-slug>
https://vault.example.com/.well-known/oauth-protected-resource/mcp/v1/vaults/<vault-slug>
https://vault.example.com/.well-known/oauth-authorization-server
https://vault.example.com/dav/v1/vaults/<vault-slug>/
```

Stop cleanly with:

```bash
docker compose --env-file .env down
```

The bind-mounted `data/` directory remains on disk. Back up `data/` together
with the separately protected master key before upgrades or restore tests.
