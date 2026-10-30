FROM python:3.12-slim@sha256:a6e34c598f2467ed0e9a8d349809fcd8b5c603269512df273a0bb1784edc11b1
RUN apt-get update && apt-get install --yes --no-install-recommends \
    mupdf-tools libglib2.0-0 libgl1 libgomp1 ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY tools/requirements-extraction.txt /tmp/constraints.txt
RUN pip install --no-cache-dir --index-url https://download.pytorch.org/whl/cpu \
    torch==2.8.0 torchvision==0.23.0 \
    && pip install --no-cache-dir -c /tmp/constraints.txt marker-pdf==2.0.0 jsonschema==4.25.1 \
    && pip freeze > /runtime-requirements.txt
COPY tools/*.py /tools/
COPY schemas/source-kinds.json schemas/source-leaf-kinds.json /schemas/
ENV TORCH_DEVICE=cpu HF_HOME=/cache/huggingface XDG_CACHE_HOME=/cache \
    FONT_DIR=/cache/fonts FONT_PATH=/cache/fonts/GoNotoCurrent-Regular.ttf
WORKDIR /tools
ENTRYPOINT ["python", "/tools/extract_manual.py"]
