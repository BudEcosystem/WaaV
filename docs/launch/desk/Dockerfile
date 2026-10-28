# The demo agent's computer: a sandboxed Linux desktop (virtual screen, window manager, terminal,
# Chromium, LibreOffice Writer) that the agent drives through the demo's tool server.
FROM debian:bookworm
ENV DEBIAN_FRONTEND=noninteractive
RUN printf 'Acquire::http::Timeout "25";\nAcquire::Retries "8";\n' > /etc/apt/apt.conf.d/80retries \
    && apt-get update && apt-get install -y --no-install-recommends \
      xvfb fluxbox xterm tmux chromium libreoffice-writer libreoffice-gtk3 \
      fonts-dejavu fonts-noto-core fonts-inter curl ca-certificates ffmpeg x11-utils xdotool wmctrl \
      python3 python3-pip procps jq tar coreutils less \
    && rm -rf /var/lib/apt/lists/*
RUN useradd -m -s /bin/bash ava
USER ava
WORKDIR /home/ava
COPY --chown=ava:ava start.sh /home/ava/start.sh
CMD ["/bin/bash", "/home/ava/start.sh"]
