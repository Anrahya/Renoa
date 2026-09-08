FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends ripgrep ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY renoa-workspace-tool /usr/local/bin/renoa-workspace-tool
USER 65534:65534
WORKDIR /workspace
CMD ["sleep", "infinity"]
