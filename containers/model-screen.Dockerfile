# Optional transfer tooling only; build from the empty context directory.
FROM mimik-decision-kev:20261015
RUN apt-get update && apt-get install -y --no-install-recommends aria2 \
    && rm -rf /var/lib/apt/lists/*
