#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
# Reuse the already-running project location rather than remounting a new checkout.
if ! active=$(docker inspect mcp-lgtm-minimal-mcp-js-1 --format '{{index .Config.Labels "com.docker.compose.project.working_dir"}}' 2>/dev/null); then
 active=$(sudo -n docker inspect mcp-lgtm-minimal-mcp-js-1 --format '{{index .Config.Labels "com.docker.compose.project.working_dir"}}' 2>/dev/null || true)
fi
if [[ -n "$active" && "$active" != "$PWD" && -f "$active/demo.sh" ]]; then
 exec "$active/demo.sh" "$@"
fi
docker_cmd=(docker)
if ! docker info >/dev/null 2>&1; then docker_cmd=(sudo docker); fi
case "${1:-status}" in
 status) "${docker_cmd[@]}" compose ps ;;
 up) "${docker_cmd[@]}" compose up -d --build ;;
 seed) exec python3 seed.py ;;
 agent) shift; exec ./agent.sh "$@" ;;
 reset)
  python3 - <<'PY'
from pathlib import Path
import datetime,shutil
archive=Path('.evidence')/('dashboards-'+datetime.datetime.now(datetime.timezone.utc).strftime('%Y%m%dT%H%M%S%fZ'));archive.mkdir(parents=True)
for p in Path('dashboards').glob('*.json'):shutil.move(str(p),str(archive/p.name))
print('Dashboards archived at',archive,'; existing COW sessions and telemetry preserved. Start a fresh Pi conversation for empty scratch.')
PY
  exec python3 seed.py ;;
 *) echo 'Usage: ./demo.sh status|up|seed|agent|reset'; exit 2 ;;
esac
