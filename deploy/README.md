# Private VPS deployment

This directory contains two independent private services:

- `renoa-coordinator` carries RCP task continuity; and
- `renoa-registry` shares immutable Agent Plugin packages between Hosts.

Neither service is required to run the other. Both remain plaintext and
loopback-only behind Tailscale Serve. Tailscale is the current private route,
not part of either Renoa protocol. Funnel is not used.

## RCP coordinator

Build the Linux binary with the workspace's locked dependencies:

```sh
cargo build --locked --release -p renoa-control --bin renoa-coordinator
```

Install `target/release/renoa-coordinator` at
`/usr/local/bin/renoa-coordinator`, copy `renoa-coordinator.service` to
`/etc/systemd/system/`, then enable the service:

```sh
systemctl daemon-reload
systemctl enable --now renoa-coordinator.service
```

Expose the loopback listener on a private tailnet HTTPS port:

```sh
tailscale serve --bg --yes --https=8443 http://127.0.0.1:7818
```

RCP peers then connect to:

```text
wss://<vps-magic-dns-name>:8443/connect
```

If tailnet certificate issuance is temporarily unavailable, a private HTTP
Serve endpoint can prove continuity without exposing a public port:

```sh
tailscale serve --bg --yes --http=8081 http://127.0.0.1:7818
```

Peers then use `ws://<vps-magic-dns-name>:8081/connect`. Tailscale still
encrypts the network path, but browser secure-context rules may require WSS;
the HTTP endpoint is a temporary fallback, not the target deployment.

Verify both layers independently:

```sh
systemctl status renoa-coordinator.service
tailscale serve status
```

The service runs as a dynamic user. Systemd creates `/var/lib/renoa` with mode
`0700`, and the service umask keeps the SQLite journal owner-only. Run local
bootstrap commands inside a transient systemd sandbox so they see the same
protected state directory:

```sh
systemd-run --quiet --wait --pipe --collect \
  --property=DynamicUser=yes \
  --property=StateDirectory=renoa \
  --property=StateDirectoryMode=0700 \
  --property=UMask=0077 \
  /usr/local/bin/renoa-coordinator enroll-surface \
  /var/lib/renoa/control.sqlite <principal-uuid> <surface-name>
```

Its JSON output contains a single-use secret that expires after five minutes.
Use the same wrapper to enroll the execution node and create its task binding:

```sh
systemd-run --quiet --wait --pipe --collect \
  --property=DynamicUser=yes \
  --property=StateDirectory=renoa \
  --property=StateDirectoryMode=0700 \
  --property=UMask=0077 \
  /usr/local/bin/renoa-coordinator enroll-node \
  /var/lib/renoa/control.sqlite <node-uuid>

systemd-run --quiet --wait --pipe --collect \
  --property=DynamicUser=yes \
  --property=StateDirectory=renoa \
  --property=StateDirectoryMode=0700 \
  --property=UMask=0077 \
  /usr/local/bin/renoa-coordinator create-task \
  /var/lib/renoa/control.sqlite \
  <task-uuid> <principal-uuid> <node-uuid> <target>
```

Enrollment output is secret. Capture it directly into an owner-only file and
exchange it immediately. These local commands do not create a remote
administration protocol.

## Current proof status

On 2026-08-11, Tailscale's DNS-01 record reached its authoritative nameservers,
but Let's Encrypt's secondary validator still received `NXDOMAIN`. The broken
HTTPS listener was disabled to avoid consuming more authorization retries. The
VPS currently exposes only the tailnet-private port `8081` fallback above;
Funnel remains disabled.

On 2026-08-12, coordinator binary
`3918d12d6ee2f40307b3a7177227e243d2add2afdec67144ee8d31cf9d8cb557`
was deployed. A trusted bootstrap created a fresh principal, Pi node, and task.
The Mac node used Pi SDK, SuperGrok, and `grok-4.5` to read and edit one confined
workspace file. The attached TypeScript surface disconnected immediately after
command admission and reconnected only after the node had durably published its
terminal event. It received a contiguous 13-event task history, 12 events by
replay, one command admission, and one completed terminal. The coordinator
remained loopback-only, and the proof used the tailnet-only port above.

## Shared Agent Plugin registry

The registry is not a remote Host or an Agent runtime. It stores only immutable
package archives and their ordered revisions. Credentials, MCP connections,
profile attachments, workspaces, and sessions remain on each Host.

Build its Linux binary from the locked workspace:

```sh
cargo build --locked --release -p renoa-registry --bin renoa-registry
```

Install `target/release/renoa-registry` at
`/usr/local/bin/renoa-registry`, copy `renoa-registry.service` to
`/etc/systemd/system/`, and enable it:

```sh
systemctl daemon-reload
systemctl enable --now renoa-registry.service
```

Expose only its loopback listener to the private tailnet. Use private HTTPS when
certificate issuance works:

```sh
tailscale serve --bg --yes --https=8444 http://127.0.0.1:7820
```

The current private HTTP fallback is:

```sh
tailscale serve --bg --yes --http=8082 http://127.0.0.1:7820
```

The registry v1 intentionally has no second application login. Tailnet
membership and ACLs are its first deployment boundary, so do not expose this
port through Funnel or a public reverse proxy. Verify service and route before
configuring a Host:

```sh
systemctl status renoa-registry.service
tailscale serve status
curl --fail --show-error http://<vps-magic-dns-name>:8082/v1/status
```

Then add the origin—not `/v1`—to every trusted Host process:

```sh
export RENOA_SHARED_PLUGIN_REGISTRY='http://<vps-magic-dns-name>:8082/'
renoa-agent plugins sync
```

The sync command's JSON reports local publications, downloads, and the durable
applied revision. The first successful response binds that Host data directory
to the registry's stable UUID. Changing the URL is safe when it routes to the
same registry state; pointing it at another registry fails closed.

On 2026-08-31, registry binary
`384752de4a643b6f6da0ae66828a45bcfb50cf816f07ebcd5ed167fd16dee9e2`
was built with Rust 1.95 on Debian Bookworm and deployed beside the existing
coordinator. The service remained IPv4-loopback-only on port `7820`; Tailscale
Serve exposed the tailnet-only HTTP fallback on port `8082`. A forced service
restart preserved the registry identity and empty revision log before its first
Host sync. The existing laptop Host then published eight immutable package
revisions; a fresh second Host pulled all eight over the tailnet and durably
advanced to revision `8`. The hardened systemd unit scored `1.3 OK` under
`systemd-analyze security` on that host.
