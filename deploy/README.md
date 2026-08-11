# Private VPS deployment

This is Renoa's first personal deployment proof, not a required RCP topology.
The coordinator remains plaintext and loopback-only. The target Tailscale Serve
endpoint terminates TLS inside the tailnet; the temporary private HTTP fallback
below is used when tailnet certificate issuance is unavailable. Tailscale
Funnel is not used.

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
