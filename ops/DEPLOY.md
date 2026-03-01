# Deployment Guide (AOT)

This page defines canonical deployment patterns for AOT binaries.

## Document contract

- `Normative`: Yes (operational patterns and release image path).
- `Front door`: No. Start onboarding from `../README.md`.
- `Owned concerns`: VM/Docker/systemd/Kubernetes deployment conventions for AOT artifacts.
- `Conflict policy`: runtime behavior semantics defer to `../spec/runtime.md`; release policy
  defer to `RELEASE.md` and `AOT_RELEASE_CONTRACT.md`.

## Official container image path

Canonical image path:

- `ghcr.io/dmitrijkiltau/fuse-aot-demo:<tag>`

Source of truth:

- built from release artifact `fuse-aot-linux-x64.tar.gz`
- published by `.github/workflows/release-artifacts.yml` on tagged releases (`v*`)

Tag policy:

- release tag mirror: `vX.Y.Z`
- immutable commit tag: `<git-sha>`

## Canonical minimal production Dockerfile

Reference file: `ops/docker/AOT_MINIMAL.Dockerfile`

Use it from your app package root after `fuse build --release` produced `.fuse/build/program.aot`.

```bash
./scripts/fuse build --manifest-path /path/to/your-app --release
docker build -f ops/docker/AOT_MINIMAL.Dockerfile -t your-app:prod /path/to/your-app
docker run --rm -p 3000:3000 -e PORT=3000 -e FUSE_HOST=0.0.0.0 your-app:prod
```

Health route convention:

- runtime does not auto-register `/health`
- canonical service route pattern:
  `get "/health" -> Map<String, String>: return {"status": "ok"}`

## VM deployment (binary + service user)

Minimal pattern:

1. Build artifact:
   - `./scripts/fuse build --manifest-path /path/to/app --release`
2. Copy binary:
   - source: `/path/to/app/.fuse/build/program.aot`
   - destination: `/opt/fuse/<app>/app-aot`
3. Create runtime user and ownership:
   - user: `fuseapp` (non-root)
4. Set runtime env:
   - `PORT`, `FUSE_HOST`, `FUSE_CONFIG`, app-specific vars
5. Start process under service manager (`systemd` preferred).

## Docker deployment

Two canonical Docker options:

1. App-local Dockerfile path:
   - build from local AOT output via `ops/docker/AOT_MINIMAL.Dockerfile`
2. Official reference image:
   - `ghcr.io/dmitrijkiltau/fuse-aot-demo:<tag>`
   - built from release artifacts for reproducible smoke deployments

Release artifact image build command (maintainers):

```bash
./scripts/package_aot_container_image.sh \
  --archive dist/fuse-aot-linux-x64.tar.gz \
  --image ghcr.io/dmitrijkiltau/fuse-aot-demo \
  --tag v0.7.0 \
  --tag "$(git rev-parse --short=12 HEAD)" \
  --push
```

## systemd deployment

Example unit:

```ini
[Unit]
Description=Fuse AOT Service
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=fuseapp
Group=fuseapp
WorkingDirectory=/opt/fuse/app
Environment=PORT=3000
Environment=FUSE_HOST=0.0.0.0
Environment=FUSE_CONFIG=/etc/fuse/app-config.toml
ExecStart=/opt/fuse/app/app-aot
Restart=on-failure
RestartSec=2
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true

[Install]
WantedBy=multi-user.target
```

Reload + enable:

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now fuse-app.service
sudo systemctl status fuse-app.service
```

## Kubernetes deployment

Minimal Deployment + Service pattern:

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: fuse-app
spec:
  replicas: 2
  selector:
    matchLabels:
      app: fuse-app
  template:
    metadata:
      labels:
        app: fuse-app
    spec:
      containers:
        - name: app
          image: your-registry/your-app:prod
          ports:
            - containerPort: 3000
          env:
            - name: PORT
              value: "3000"
            - name: FUSE_HOST
              value: "0.0.0.0"
          readinessProbe:
            httpGet:
              path: /health
              port: 3000
          livenessProbe:
            httpGet:
              path: /health
              port: 3000
---
apiVersion: v1
kind: Service
metadata:
  name: fuse-app
spec:
  selector:
    app: fuse-app
  ports:
    - port: 80
      targetPort: 3000
```

If your app uses a non-`/health` route prefix, adjust probe paths explicitly.
