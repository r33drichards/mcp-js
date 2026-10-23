#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
case "${1:-status}" in
 status) docker compose ps ;;
 up) docker compose up -d --build ;;
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
