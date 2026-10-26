# Laya (Convai Innovations, Apache-2.0) prototype runtime for the field-reference experiment.
# Build: docker build -t mimik-laya:0.4.15 -f containers/laya.Dockerfile <dir> (a directory holding
# only requirements-laya.txt). Run with --network none and the models mounted read-only.
FROM python@sha256:a6e34c598f2467ed0e9a8d349809fcd8b5c603269512df273a0bb1784edc11b1
COPY requirements-laya.txt /tmp/requirements-laya.txt
RUN pip install --no-cache-dir --extra-index-url https://download.pytorch.org/whl/cpu \
      -r /tmp/requirements-laya.txt && rm /tmp/requirements-laya.txt
ENV HF_HUB_OFFLINE=1 TRANSFORMERS_OFFLINE=1 USE_TF=0 PYTHONDONTWRITEBYTECODE=1
USER 1000:1000
