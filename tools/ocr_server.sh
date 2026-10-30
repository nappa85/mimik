#!/bin/sh
# Optional prototype: serve surya's OCR model for tools/extract_manual.py --ocr-server.
#
# Runs the pinned llama.cpp llama-server (b11609, the build Mimik ships) in a container on
# the internal Docker network mimik-ocr-net (no outside access), with surya-ocr-2 at a pinned
# revision, hash-checked before start. DEVICE=gpu uses the Vulkan build and is refused on
# weekdays 09:00-18:00 (the machine schedule; the fan must be on manual); the image encoder
# stays on the CPU there, because the container's Mesa 25.0.7 RADV lost the device on it
# (2026-10-29). DEVICE=cpu is the default: 2 cores on weekdays 09:00-18:00, else CPUS
# (default 6). The server does not stop itself: run "stop" after the extraction.
#
# usage: tools/ocr_server.sh start|stop
#   LLAMA_DIR: the verified runtime's llama/ directory (cpu/ and vulkan/)
#   MODEL_DIR: surya-2.gguf and surya-2-mmproj.gguf of datalab-to/surya-ocr-2-gguf at revision
#              6a3a4c30e5e74446d4f8b6afd05b2f2da970f470 (modified OpenRAIL-M)
set -eu

NAME=mimik-ocr-llama
NETWORK=mimik-ocr-net
IMAGE=mimik-model-screen:20261015
MODEL_SHA256=1f18abe17b1ed8b4e47ee9b1ad0e274c93daf5efbb6b29a04ff1712e37051e05
MMPROJ_SHA256=98c0563673b1657ff6d021d1e5f04af06cbf61bb40c63ac613e8bb71b42fb2c0

case "${1:-}" in
stop)
    docker rm -f "$NAME" >/dev/null
    exit 0
    ;;
start) ;;
*)
    echo "usage: $0 start|stop" >&2
    exit 2
    ;;
esac

: "${LLAMA_DIR:?set LLAMA_DIR}" "${MODEL_DIR:?set MODEL_DIR}"
check() {
    actual=$(sha256sum "$MODEL_DIR/$1" | cut -d' ' -f1)
    [ "$actual" = "$2" ] || { echo "hash mismatch: $1" >&2; exit 1; }
}
check surya-2.gguf "$MODEL_SHA256"
check surya-2-mmproj.gguf "$MMPROJ_SHA256"

# Weekdays 09:00-18:00: no GPU, at most 2 cores.
day=$(date +%u)
hour=$(date +%H)
working_hours=false
if [ "$day" -le 5 ] && [ "${hour#0}" -ge 9 ] && [ "${hour#0}" -lt 18 ]; then
    working_hours=true
fi
cpus=${CPUS:-6}
if [ "$working_hours" = true ]; then
    [ "${DEVICE:-cpu}" = gpu ] && { echo "no GPU on weekdays 09:00-18:00" >&2; exit 1; }
    cpus=2
fi

if docker network inspect "$NETWORK" >/dev/null 2>&1; then
    internal=$(docker network inspect --format '{{.Internal}}' "$NETWORK")
    [ "$internal" = true ] || { echo "$NETWORK exists but is not internal" >&2; exit 1; }
else
    docker network create --internal "$NETWORK" >/dev/null
fi
set -- --network "$NETWORK" --cap-drop=ALL --security-opt no-new-privileges \
    --cpus "$cpus" --memory 8g --pids-limit 256 \
    --user "$(id -u):$(id -g)" -e HOME=/tmp -v "$MODEL_DIR:/m:ro"
if [ "${DEVICE:-cpu}" = gpu ]; then
    set -- "$@" --device /dev/dri --group-add "$(getent group video | cut -d: -f3)" \
        --group-add "$(getent group render | cut -d: -f3)" \
        -v "$LLAMA_DIR/vulkan:/llama:ro" -e LD_LIBRARY_PATH=/llama
    offload="-ngl 99 --no-mmproj-offload"
else
    set -- "$@" -v "$LLAMA_DIR/cpu:/llama:ro" -e LD_LIBRARY_PATH=/llama
    offload="-ngl 0"
fi
# shellcheck disable=SC2086 # offload holds separate flags
docker run -d --name "$NAME" "$@" --entrypoint /llama/llama-server "$IMAGE" \
    -m /m/surya-2.gguf --mmproj /m/surya-2-mmproj.gguf $offload -t "$cpus" --host 0.0.0.0 --port 8080 \
    --parallel 4 --ctx-size 49152 --alias datalab-to/surya-ocr-2 --jinja >/dev/null
echo "OCR server $NAME on $NETWORK: pass --ocr-server http://$NAME:8080 and --network $NETWORK"
