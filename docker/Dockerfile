# Use Debian bullseye-slim as the base image for newer OpenSSL
FROM ubuntu:24.04
ARG CRATE_VERSION
ARG TARGETARCH

# Set maintainer metadata
LABEL maintainer="rustmailer <rustmailer.git@gmail.com>"
LABEL version="${CRATE_VERSION}"
LABEL description="Dockerized Bichon service"

# Set working directory
WORKDIR /opt/bichon

# Copy compiled binary (ensure it's statically linked or compatible with bullseye)
COPY ${TARGETARCH}/bichon-server /opt/bichon/bichon-server
COPY ${TARGETARCH}/LICENSE /opt/bichon/
# Third-party notices for every binary shipped in this image.
COPY ${TARGETARCH}/third-party-licenses/ /opt/bichon/third-party-licenses/


# cli tools
COPY ${TARGETARCH}/bichon-cli /usr/local/bin/bichon-cli
COPY ${TARGETARCH}/bichon-admin /usr/local/bin/bichon-admin

# Set permissions
RUN chmod +x /opt/bichon/bichon-server
RUN chmod +x /usr/local/bin/bichon-cli
RUN chmod +x /usr/local/bin/bichon-admin


# Install ca-certificates to ensure HTTPS certificate verification works correctly.
# tini becomes PID 1 so any process reparented to it (the healthcheck runs
# through a shell) is reaped instead of accumulating as a zombie.
RUN apt update && apt install -y ca-certificates curl tini && rm -rf /var/lib/apt/lists/*
# Create data directory
RUN mkdir -p /data

# Expose default ports
EXPOSE 15630

# Set volume and working directory
WORKDIR /data

# --max-time stays below the healthcheck timeout so curl exits on its own
# instead of being orphaned when Docker kills a timed-out check.
HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
  CMD curl -fs --max-time 4 http://localhost:${BICHON_HTTP_PORT:-15630}/api/status || exit 1

# tini as PID 1: reaps reparented children and forwards SIGTERM to the server.
ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["/opt/bichon/bichon-server"]
