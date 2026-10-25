"""Guest-side acceptance tests; execution, admission and Raft are never mocked.

The HTTP probe observes JS execution and lets a test release it normally. It is
not a worker or an admission service. Cancellation is a required contract: an
unimplemented cancel command fails the test rather than skipping it.
"""

import argparse
import datetime
import http.server
import json
import os
from pathlib import Path
import queue
import subprocess
import threading
import time
import urllib.error
import urllib.request


def eventually(check, timeout=30):
    deadline = time.monotonic() + timeout
    last = None
    while time.monotonic() < deadline:
        last = check()
        if last:
            return last
        time.sleep(0.1)
    raise AssertionError(f"condition timed out after {timeout}s; last value: {last!r}")


class Probe(http.server.BaseHTTPRequestHandler):
    lock = threading.Lock()
    starts = {}
    pulses = {}
    completed = set()
    released = set()

    def do_GET(self):
        kind, name, identity = self.path.strip("/").split("/")
        key = (name, identity)
        with self.lock:
            if kind == "start":
                self.starts.setdefault(name, set()).add(identity)
            elif kind == "pulse":
                self.pulses[key] = self.pulses.get(key, 0) + 1
            elif kind == "done":
                self.completed.add(key)
            else:
                self.send_error(404)
                return
            body = json.dumps({"released": key in self.released}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_):
        pass


class Runner:
    def __init__(self, executable, config, env, root, number):
        self.stderr = (root / f"runner-{number}.stderr").open("w")
        self.transcript = (root / f"runner-{number}.jsonl").open("w")
        self.process = subprocess.Popen(
            [executable, str(config)], stdin=subprocess.PIPE,
            stdout=subprocess.PIPE, stderr=self.stderr, env=env, text=True,
            bufsize=1,
        )
        self.lines = queue.Queue()

        def read():
            for line in self.process.stdout:
                self.transcript.write(line)
                self.transcript.flush()
                self.lines.put(line)
            self.lines.put(None)

        threading.Thread(target=read, daemon=True).start()

    def command(self, action, **fields):
        assert self.process.poll() is None, "runner exited; inspect guest stderr"
        self.process.stdin.write(json.dumps({"action": action, **fields}) + "\n")
        self.process.stdin.flush()
        line = self.lines.get(timeout=15)
        assert line is not None, "runner stdout closed"
        return json.loads(line)

    def mutate(self, action, **fields):
        result = self.command(action, **fields)
        assert "error" not in result, (action, result)
        return result


class Cluster:
    def __init__(self, args):
        self.root = Path("/tmp/scheduler-integration")
        self.root.mkdir(parents=True, exist_ok=False)
        self.processes = []
        self.runners = []
        self.probe = http.server.ThreadingHTTPServer(("127.0.0.1", 8090), Probe)
        threading.Thread(target=self.probe.serve_forever, daemon=True).start()
        self.env = dict(os.environ, MCP_SCHEDULER_WORKER_TOKEN="vm-only-worker",
                        MCP_SCHEDULER_ADMISSION_TOKEN="vm-only-admission")
        expires = (datetime.datetime.now(datetime.timezone.utc) +
                   datetime.timedelta(days=1)).isoformat()
        worker = {
            "namespace": "nixos-integration", "worker_id": "worker",
            "data_dir": str(self.root / "worker-store"),
            "bearer_env": "MCP_SCHEDULER_WORKER_TOKEN",
            "admission_bearer_env": "MCP_SCHEDULER_ADMISSION_TOKEN",
            "admission_urls": [f"http://127.0.0.1:{4500+i}" for i in range(1, 4)],
        }
        self.write("worker.json", worker)
        worker_env = dict(self.env, MCP_SCHEDULED_WORKER_CONFIG=str(self.root / "worker.json"))
        log = (self.root / "worker.log").open("w")
        process = subprocess.Popen([args.server, "--http-port", "8080"], env=worker_env,
                                   stdout=log, stderr=subprocess.STDOUT, cwd=self.root)
        self.processes.append(process)
        eventually(lambda: self.http_status("/api/scheduled/v1/executions/not-found") == 404)
        for i in range(1, 4):
            config = {
                "node_id": f"r{i}", "advertise_addr": f"127.0.0.1:{4400+i}",
                "cluster_port": 4400+i,
                "peers": [f"r{j}@127.0.0.1:{4400+j}" for j in range(1, 4) if j != i],
                "data_dir": str(self.root / f"r{i}-store"),
                "admission_addr": f"127.0.0.1:{4500+i}",
                "admission_bearer_env": "MCP_SCHEDULER_ADMISSION_TOKEN",
                "schedules": [],
                "remote": {
                    "url": "http://127.0.0.1:8080", "namespace": "nixos-integration",
                    "worker_id": "worker", "bearer_env": "MCP_SCHEDULER_WORKER_TOKEN",
                    "expires_at": expires, "session": "nixos-scheduler",
                    "timeout_seconds": 240, "heap_mb": 64,
                },
            }
            path = self.write(f"r{i}.json", config)
            runner = Runner(args.runner, path, self.env, self.root, i)
            self.runners.append(runner)
            self.processes.append(runner.process)
        self.owner = eventually(self.find_owner)

    def write(self, name, value):
        path = self.root / name
        path.write_text(json.dumps(value))
        return path

    def http_status(self, path, token="vm-only-worker", method="GET"):
        request = urllib.request.Request("http://127.0.0.1:8080" + path,
                                         headers={"Authorization": "Bearer " + token},
                                         method=method)
        try:
            with urllib.request.urlopen(request, timeout=2) as response:
                return response.status
        except urllib.error.HTTPError as error:
            return error.code
        except urllib.error.URLError:
            return None

    def find_owner(self):
        for runner in self.runners:
            if runner.process.poll() is None:
                if runner.command("status").get("owner_acquired"):
                    return runner
        return None

    def view(self):
        result = self.owner.mutate("list")
        assert result["stale_local_view"] is True
        return result

    def schedule(self, name):
        # Each invocation has a distinct probe identity, so a later successful
        # invocation cannot mask the original execution continuing after cancel.
        code = """
            const id = Date.now().toString() + '-' + Math.random().toString();
            const base = 'http://127.0.0.1:8090/';
            const suffix = NAME + '/' + id;
            await fetch(base + 'start/' + suffix);
            while (!(await (await fetch(base + 'pulse/' + suffix)).json()).released) {
                await new Promise(resolve => setTimeout(resolve, 100));
            }
            await fetch(base + 'done/' + suffix);
        """.replace("NAME", json.dumps(name))
        return self.owner.mutate("create", definition={
            "name": name, "code": code, "cron": "* * * * *", "timezone": "UTC",
            "policy": {"missed": "Skip", "overlap": "Skip",
                       "retry": {"max_retries": 0, "delay_seconds": 0}},
        })["id"]

    def tick(self):
        # Advance guest wall time to a real cron boundary; no fake runner clock.
        target = int(time.time() // 60 + 1) * 60 + 1
        subprocess.run(["date", "-u", "-s", f"@{target}"], check=True)

    def running(self, schedule_id, name, excluded=()):
        def started():
            with Probe.lock:
                identities = Probe.starts.get(name, set()) - set(excluded)
                active = [i for i in identities if Probe.pulses.get((name, i), 0) >= 2]
            occurrences = [o for o in self.view()["occurrences"]
                           if o["schedule_id"] == schedule_id and "Running" in o["status"]]
            return (occurrences[-1], active[-1]) if occurrences and active else None
        return eventually(started)

    def terminal(self, occurrence_id, expected):
        def finished():
            occurrence = next(o for o in self.view()["occurrences"] if o["id"] == occurrence_id)
            return occurrence if expected in occurrence["status"] else None
        return eventually(finished)

    def enabled(self, schedule_id):
        return next(s for s in self.view()["schedules"] if s["id"] == schedule_id)["enabled"]

    def release(self, name, identity):
        with Probe.lock:
            Probe.released.add((name, identity))

    def close(self):
        for process in reversed(self.processes):
            if process.poll() is None:
                process.terminate()
        for process in reversed(self.processes):
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
        if hasattr(self, "probe"):
            self.probe.shutdown()
            self.probe.server_close()


def disable_only(cluster):
    name = "disable-only"
    schedule = cluster.schedule(name)
    cluster.tick()
    occurrence, identity = cluster.running(schedule, name)
    cluster.owner.mutate("disable", id=schedule)
    assert not cluster.enabled(schedule)
    # Disabling future runs must not cancel accepted execution.
    cluster.release(name, identity)
    cluster.terminal(occurrence["id"], "Succeeded")
    cluster.tick()
    time.sleep(3)
    with Probe.lock:
        assert Probe.starts[name] == {identity}, "disabled schedule launched another run"
        assert (name, identity) in Probe.completed


def cancellation(cluster):
    name = "cancel-only"
    schedule = cluster.schedule(name)
    cluster.tick()
    occurrence, identity = cluster.running(schedule, name)
    # This command is deliberately a hard requirement, not an xfail/skip.
    cluster.owner.mutate("cancel", occurrence_id=occurrence["id"])
    cluster.terminal(occurrence["id"], "Cancelled")
    assert cluster.enabled(schedule), "cancellation disabled the schedule"
    with Probe.lock:
        before = Probe.pulses[(name, identity)]
    time.sleep(2)
    with Probe.lock:
        assert Probe.pulses[(name, identity)] == before, "cancelled JS is still executing"
        assert (name, identity) not in Probe.completed
    # A duplicate cancellation must not rearm the invocation or disable its schedule.
    cluster.owner.mutate("cancel", occurrence_id=occurrence["id"])
    assert cluster.enabled(schedule)
    cluster.tick()
    later, later_identity = cluster.running(schedule, name, excluded={identity})
    assert later["id"] != occurrence["id"]
    cluster.release(name, later_identity)
    cluster.terminal(later["id"], "Succeeded")
    cluster.owner.mutate("disable", id=schedule)


def disabled_cancellation(cluster):
    name = "disable-then-cancel"
    schedule = cluster.schedule(name)
    cluster.tick()
    occurrence, identity = cluster.running(schedule, name)
    cluster.owner.mutate("disable", id=schedule)
    cluster.owner.mutate("cancel", occurrence_id=occurrence["id"])
    cluster.terminal(occurrence["id"], "Cancelled")
    assert not cluster.enabled(schedule)
    cluster.tick()
    time.sleep(2)
    with Probe.lock:
        assert Probe.starts[name] == {identity}
        assert (name, identity) not in Probe.completed


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--server", required=True)
    parser.add_argument("--runner", required=True)
    parser.add_argument("--scenario", choices=["disable", "cancellation", "disabled-cancellation"], required=True)
    args = parser.parse_args()
    cluster = Cluster.__new__(Cluster)
    try:
        cluster.__init__(args)
        {"disable": disable_only, "cancellation": cancellation,
         "disabled-cancellation": disabled_cancellation}[args.scenario](cluster)
        print(f"PASS: {args.scenario}", flush=True)
    finally:
        if hasattr(cluster, "processes"):
            cluster.close()


if __name__ == "__main__":
    main()
