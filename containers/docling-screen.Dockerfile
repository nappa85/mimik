FROM python:3.12-slim@sha256:a6e34c598f2467ed0e9a8d349809fcd8b5c603269512df273a0bb1784edc11b1
RUN apt-get update && apt-get install --yes --no-install-recommends \
    libglib2.0-0 libgl1 libgomp1 ca-certificates \
    && rm -rf /var/lib/apt/lists/*
RUN pip install --no-cache-dir --extra-index-url https://download.pytorch.org/whl/cpu \
    docling==2.139.0 onnxruntime==1.31.0 \
    && pip freeze > /runtime-requirements.txt
ENV HF_HOME=/cache/huggingface XDG_CACHE_HOME=/cache HOME=/cache
WORKDIR /work
