# Docker Image

The official image is published to GitHub Container Registry:

```sh
docker pull ghcr.io/wavefunk/sendword:latest
```

Run with a mounted data directory:

```sh
mkdir -p data
docker run --rm -p 8080:8080 -v "$PWD/data:/data" ghcr.io/wavefunk/sendword:latest
```

The image sets `SENDWORD_SERVER__BIND=0.0.0.0` so the published port accepts
traffic from the host. The container runs as the `sendword` user with UID/GID
1000 so commands and webhook scripts do not run as root.

The image includes:

- `sendword`
- Node.js and npm
- Python 3 and pip

Those runtimes are available to webhook scripts and commands that execute inside the container.

## Local Smoke Test

```sh
docker build -t sendword:local .
docker run --rm sendword:local sendword --help
docker run --rm sendword:local node --version
docker run --rm sendword:local python3 --version
```
