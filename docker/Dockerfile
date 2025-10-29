# Minimal StarCraft II headless image (no Node.js, no auto-start)
# Build: docker build -t minimal-sc2 .
# Usage (manual run):
#   docker run --rm -it minimal-sc2 /StarCraftII/Versions/Base*/SC2_x64 \
#     -listen 0.0.0.0 -port 8167 -displayMode 0 -dataDir /StarCraftII
# Accepts EULA via unzip password.

ARG SC2_VERSION=4.10

FROM debian:bookworm-slim AS downloader
ARG SC2_VERSION
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update \
    && apt-get install -y --no-install-recommends wget unzip ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /
# Download official Linux SC2 package (static version 4.10 used widely for AI ladders)
RUN wget -q http://blzdistsc2-a.akamaihd.net/Linux/SC2.${SC2_VERSION}.zip \
    && unzip -q -P iagreetotheeula SC2.${SC2_VERSION}.zip \
    && rm SC2.${SC2_VERSION}.zip \
    # Remove unneeded content to shrink size
    && rm -rf /StarCraftII/Battle.net/* \
    && rm -rf /StarCraftII/Versions/Shaders* \
    && rm -rf /StarCraftII/Maps

# Identify required shared libraries for SC2_x64 (informational)
RUN ldd /StarCraftII/Versions/Base*/SC2_x64 | awk '/=>/ {print $3}' | sort -u > /required-libs.txt || true

FROM debian:bookworm-slim AS runtime
ENV DEBIAN_FRONTEND=noninteractive
# Install only libraries SC2 relies on; using a minimal curated set.
# (If SC2 fails to start, run ldd inside a temporary container and add missing libs.)
RUN apt-get update \
     && apt-get install -y --no-install-recommends \
         libatomic1 libstdc++6 libgcc-s1 libtinfo6 libncursesw6 libzstd1 libbz2-1.0 \
         libcurl4 libxml2 libsm6 libice6 libxrender1 libxrandr2 libxi6 libxext6 libxfixes3 libxcursor1 libxdamage1 libxcomposite1 libx11-6 libxxf86vm1 libdrm2 libgl1-mesa-glx libglib2.0-0 \
     && rm -rf /var/lib/apt/lists/* \
     && useradd -m -u 1000 sc2

COPY --from=downloader /StarCraftII /StarCraftII
# (Optional) copy list of required libs for debugging
COPY --from=downloader /required-libs.txt /StarCraftII/required-libs.txt
 # Helper script (manual invocation)
COPY run-sc2.sh /StarCraftII/run-sc2.sh
COPY healthcheck-sc2.sh /StarCraftII/healthcheck-sc2.sh
RUN chmod +x /StarCraftII/run-sc2.sh
RUN chmod +x /StarCraftII/healthcheck-sc2.sh
# Ensure the game files are readable/executable by the non-root sc2 user
RUN chown -R sc2:sc2 /StarCraftII \
    && chmod -R a+rX /StarCraftII

WORKDIR /StarCraftII
USER sc2
# Expose default game port (documented; optional to map at runtime)
EXPOSE 5555
# Add ENTRYPOINT to run the helper script by default. The container will start SC2 with
# sane defaults: -listen 0.0.0.0 -port 5555. Users can override or append flags.
ENTRYPOINT ["/StarCraftII/run-sc2.sh"]
# Healthcheck: succeeds when SC2 accepts TCP connections on the configured port.
# It runs as the container user. Adjust intervals/retries as needed.
HEALTHCHECK --interval=30s --timeout=5s --start-period=20s --retries=5 \
    CMD /StarCraftII/healthcheck-sc2.sh || exit 1

# Keep a fallback CMD for interactive shells (if user wants a shell instead):
CMD ["/bin/bash"]
