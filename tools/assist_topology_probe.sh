#!/usr/bin/env bash
# Step-2b runtime topology and egress probes (prototype tier; criteria in the plan's 2b section,
# results in docs/results.md). Starts llama-server on an internal network and a pinned socat
# forwarder published only on 127.0.0.1:8091, runs the egress probes, optionally runs one live
# request per crate probe through the forwarder, and cleans up.
#
# Usage: tools/assist_topology_probe.sh <out-dir> <model-key> [probe-target-volume]
# CPU pinning: server on 0,1 (2 threads); forwarder, probes and checks on 6,7.
set -uo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
OUT=$1 KEY=$2 PROBE_VOLUME=${3:-}
FWD_IMAGE=alpine/socat@sha256:22d77becaeeea8ef56b8c4bb2312323acafe85354be6a85714ff5050e3537842
CHECK_IMAGE=sha256:9578747a34f5e02cb7561c8e2a8903b75100b96ce1b0587d5fa4b687be0d3a9f
NET=mimik-assist2b-net FRONT=mimik-assist2b-front
SRV=mimik-assist2b-srv FWD=mimik-assist2b-fwd LABEL=mimik.assist2b=topology
mkdir -p "$OUT"
R="$OUT/topology.txt"
: > "$R"
rec() { echo "$*" | tee -a "$R"; }
cleanup() {
  docker rm -f "$SRV" "$FWD" >/dev/null 2>&1
  [ -n "${LISTENER:-}" ] && kill "$LISTENER" 2>/dev/null
}
trap cleanup EXIT
docker network inspect "$NET" >/dev/null 2>&1 || docker network create --internal --label "$LABEL" "$NET" >/dev/null
docker network inspect "$FRONT" >/dev/null 2>&1 || docker network create --label "$LABEL" \
  -o com.docker.network.bridge.enable_ip_masquerade=false "$FRONT" >/dev/null
rec "net_internal=$(docker network inspect -f '{{.Internal}}' "$NET")"
rec "front_masquerade=$(docker network inspect -f '{{index .Options "com.docker.network.bridge.enable_ip_masquerade"}}' "$FRONT")"

reg() { python3 "$ROOT/tools/model_registry.py" get "$KEY" | python3 -c "import json,sys;print(json.load(sys.stdin)$1)"; }
model=$(realpath -e "/media/data/mimik/models/$(reg "['path']")")
image=$(reg "['server_image']")
mapfile -t ARGS < <(cd "$ROOT/tools" && python3 -c "
import model_registry as m
d = m.load(); e = m.model(d, '$KEY')
for a in m.server_args(e, 8192, device='cpu', threads=2): print(a)
print('--threads-batch'); print('2'); print('--reasoning'); print('off')")
docker run -d --init --pull never --name "$SRV" --label "$LABEL" --network "$NET" --network-alias server \
  --user 1000:1000 --cap-drop ALL --security-opt no-new-privileges --read-only --tmpfs /tmp \
  --memory 6g --memory-swap 6g --cpus 2 --cpuset-cpus 0,1 --pids-limit 256 \
  --mount type=bind,src="$model",dst=/models/model.gguf,readonly "$image" "${ARGS[@]}" >/dev/null
for _ in $(seq 1 600); do
  docker exec "$SRV" curl -sf http://127.0.0.1:8080/health >/dev/null 2>&1 && break
  sleep 1
done
rec "server_health=$(docker exec "$SRV" curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:8080/health)"
docker run -d --init --pull never --name "$FWD" --label "$LABEL" --network "$FRONT" \
  -p 127.0.0.1:8091:8091 --user 1000:1000 --cap-drop ALL --security-opt no-new-privileges \
  --read-only --memory 64m --pids-limit 64 --cpus 0.5 --cpuset-cpus 6 \
  "$FWD_IMAGE" TCP-LISTEN:8091,fork,reuseaddr TCP:server:8080 >/dev/null
docker network connect "$NET" "$FWD"
sleep 1
rec "fwd_networks=$(docker inspect -f '{{range $k, $v := .NetworkSettings.Networks}}{{$k}} {{end}}' "$FWD")"
rec "fwd_ports=$(docker port "$FWD")"

# Host-side listener reachable on every host address (for the "host listener" egress probe).
EMPTY=$(mktemp -d)  # serve an empty directory: nothing to leak
taskset -c 7 python3 -m http.server 18999 --bind 0.0.0.0 --directory "$EMPTY" >/dev/null 2>&1 &
LISTENER=$!
sleep 1
lan=$(ip -4 route get 1.1.1.1 2>/dev/null | awk '{for (i=1;i<=NF;i++) if ($i=="src") print $(i+1)}')
gw_int=$(docker network inspect -f '{{range .IPAM.Config}}{{.Gateway}}{{end}}' "$NET")
gw_front=$(docker network inspect -f '{{range .IPAM.Config}}{{.Gateway}}{{end}}' "$FRONT")
rec "host_lan=$lan internal_gateway=${gw_int:-none} front_gateway=$gw_front"

probe_srv() {  # probe_srv <label> <url>: from inside the model container
  local code
  code=$(docker exec "$SRV" curl -s -m 5 -o /dev/null -w '%{http_code}' "$2" 2>/dev/null; echo " exit=$?")
  rec "srv_egress $1 -> $code"
}
probe_srv public_ip_https https://1.1.1.1/
probe_srv public_ip_http http://1.1.1.1/
probe_srv public_name http://example.com/
probe_srv host_lan_listener "http://$lan:18999/"
probe_srv host_docker0 "http://172.17.0.1:18999/"
[ -n "$gw_int" ] && probe_srv internal_gateway_listener "http://$gw_int:18999/"
probe_srv front_gateway_listener "http://$gw_front:18999/"
rec "srv_dns_example=$(docker exec "$SRV" getent hosts example.com >/dev/null 2>&1; echo exit=$?)"

probe_fwd() {  # probe_fwd <label> <host:port>: raw TCP from inside the forwarder
  local rc
  docker exec "$FWD" socat -T3 /dev/null "TCP:$2,connect-timeout=3" >/dev/null 2>&1
  rc=$?
  rec "fwd_egress $1 -> exit=$rc"
}
probe_fwd public_ip_443 1.1.1.1:443
probe_fwd public_ip_80 1.1.1.1:80
probe_fwd public_name example.com:80
probe_fwd host_lan_listener "$lan:18999"
probe_fwd front_gateway_listener "$gw_front:18999"
probe_fwd upstream server:8080

through() {  # through <label> <raw request>: bytes sent to 127.0.0.1:8091, first lines kept
  local reply
  reply=$(printf '%b' "$2" | timeout 10 python3 -c "
import socket,sys
s=socket.create_connection(('127.0.0.1',8091),timeout=5); s.sendall(sys.stdin.buffer.read())
data=b''
try:
    while len(data)<4096:
        chunk=s.recv(4096)
        if not chunk: break
        data+=chunk
except Exception: pass
print(' | '.join(data.decode('latin1').splitlines()[:6]))")
  rec "through_forwarder $1 -> $reply"
}
through health 'GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n'
through absolute_form 'GET http://example.com/ HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n'
through connect 'CONNECT example.com:443 HTTP/1.1\r\nHost: example.com:443\r\n\r\n'
rec "lan_8091=$(timeout 5 curl -s -m 3 -o /dev/null -w '%{http_code}' "http://$lan:8091/health"; echo " exit=$?")"
rec "front_gateway_8091=$(timeout 5 curl -s -m 3 -o /dev/null -w '%{http_code}' "http://$gw_front:8091/health"; echo " exit=$?")"
rec "loopback_8091=$(timeout 5 curl -s -m 3 --noproxy '*' -o /dev/null -w '%{http_code}' http://127.0.0.1:8091/health; echo " exit=$?")"

if [ -n "$PROBE_VOLUME" ]; then
  (cd "$ROOT/tools" && python3 -c "
import assist_eval as ae, json
d = ae.load_cases(); c = next(x for x in d['cases'] if x['id'] == 'dh-01')
r = ae.build_request(c, d['sheet'])
json.dump({'system': r['system'], 'user': r['user']}, open('$OUT/live-request.json', 'w'))")
  for bin in p-genai p-rig; do
    docker run --rm --network host --cpus 2 --cpuset-cpus 6,7 --memory 1g --user 1000:1000 \
      --cap-drop ALL --read-only -e LIVE_URL=http://127.0.0.1:8091/v1 \
      --mount type=volume,src="$PROBE_VOLUME",dst=/t,readonly \
      --mount type=bind,src="$OUT",dst=/out "$CHECK_IMAGE" /t/release/$bin live > "$OUT/live-$bin.json" 2>&1
    rec "live $bin -> $(head -c 600 "$OUT/live-$bin.json")"
  done
fi
docker logs "$SRV" 2>&1 | grep -E "request:|POST|GET" | tail -20 > "$OUT/server-requests.txt"
rec "done"
