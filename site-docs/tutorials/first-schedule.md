# Schedule your first JavaScript run

This lesson starts one experimental runner and one dedicated worker on loopback,
then observes a minute-based occurrence. You need a checkout containing the integrated #284 hardening through #295,
Rust, Python 3, and two terminals in the repository root. Allow up to two minutes.
Read [feature status](../reference/scheduler-http-status.md): this is a local
lesson for the integrated runtime, not a production deployment recipe.

## Prepare an isolated configuration

In terminal 1:

```sh
cargo build --locked -p server --bin server
cargo build --locked --manifest-path scheduler-runner/Cargo.toml
export MCP_SCHEDULER_WORKER_TOKEN=local-tutorial-worker
export MCP_SCHEDULER_ADMISSION_TOKEN=local-tutorial-admission
export MCP_SCHEDULER_CLUSTER_TOKEN=local-tutorial-cluster
export MCP_SCHEDULE_LESSON="$(mktemp -d)"
python3 - <<'PYTHON'
import datetime, json, os
from pathlib import Path
root = Path(os.environ['MCP_SCHEDULE_LESSON'])
worker = {
    'namespace': 'lesson', 'worker_id': 'lesson-worker',
    'data_dir': str(root / 'worker-store'),
    'bearer_env': 'MCP_SCHEDULER_WORKER_TOKEN',
    'admission_bearer_env': 'MCP_SCHEDULER_ADMISSION_TOKEN',
    'admission_urls': ['http://127.0.0.1:4501']
}
runner = {
    'node_id': 'lesson-runner', 'advertise_addr': '127.0.0.1:4401',
    'cluster_port': 4401, 'peers': [], 'data_dir': str(root / 'runner-store'),
    'cluster_bearer_env': 'MCP_SCHEDULER_CLUSTER_TOKEN',
    'admission_addr': '127.0.0.1:4501',
    'admission_bearer_env': 'MCP_SCHEDULER_ADMISSION_TOKEN',
    'schedules': [],
    'remote': {
        'url': 'http://127.0.0.1:8080',
        'bearer_env': 'MCP_SCHEDULER_WORKER_TOKEN',
        'expires_at': (datetime.datetime.now(datetime.timezone.utc) +
                       datetime.timedelta(hours=1)).isoformat(),
        'session': 'lesson', 'timeout_seconds': 5, 'heap_mb': 32,
        'namespace': 'lesson', 'worker_id': 'lesson-worker'
    }
}
for name, value in [('worker', worker), ('runner', runner)]:
    (root / (name + '.json')).write_text(json.dumps(value))
print(root)
PYTHON
MCP_SCHEDULED_WORKER_CONFIG="$MCP_SCHEDULE_LESSON/worker.json"   ./target/debug/server --heap-store none --http-port 8080 --bind-host 127.0.0.1
```

These three bearer values are disposable lesson constants. Keep this experiment on
loopback; use existing managed credentials in a real installation. The worker
exposes only scheduled routes, so `/api/exec` and HTTP functions are absent.

## Start the timer runner

In terminal 2, set `MCP_SCHEDULE_LESSON` to the directory printed above:

```sh
export MCP_SCHEDULE_LESSON=/tmp/REPLACE_WITH_PRINTED_DIRECTORY
export MCP_SCHEDULER_WORKER_TOKEN=local-tutorial-worker
export MCP_SCHEDULER_ADMISSION_TOKEN=local-tutorial-admission
export MCP_SCHEDULER_CLUSTER_TOKEN=local-tutorial-cluster
scheduler-runner/target/debug/mcp-js-scheduler-runner "$MCP_SCHEDULE_LESSON/runner.json"
```

Type this line into the running runner process:

```json
{"action":"status"}
```

Repeat until the response reports `owner_acquired: true`. Election alone is not
ownership; the runner must commit acquisition before it can schedule work.

## Create and observe one schedule

Type each command into the runner, one line at a time:

```json
{"action":"create","definition":{"name":"lesson","code":"console.log('scheduled hello')","cron":"* * * * *","timezone":"UTC","policy":{"missed":"Skip","overlap":"Skip","retry":{"max_retries":0,"delay_seconds":0}}}}
{"action":"list"}
```

Creation returns a schedule `id`. The list shows its next calendar due time.
After the next minute boundary, type `list` again. An occurrence should reach
`Succeeded`; `Running` is an intermediate state. The worker executes actual
JavaScript, and the runner observes completion rather than treating HTTP 202 as
success. No host capabilities are needed for `console.log`.

## Stop future occurrences

Use the returned schedule ID (1 in fresh lesson storage):

```json
{"action":"disable","id":1}
{"action":"list"}
```

The schedule is disabled. A run already admitted may finish. Stop both processes
with Ctrl-C after observing the result. Next, learn to
[cancel a specific occurrence](../how-to/stop-scheduled-work.md) and read why
[admission does not guarantee exactly-once effects](../concepts/scheduler-admission.md).
