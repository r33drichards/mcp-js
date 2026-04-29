"""Temporal worker + REST control plane for MCP-JS background and scheduled
JavaScript task execution.

The worker hosts two workflows backed by a single activity that calls back
into the MCP-JS server's `/api/exec` HTTP endpoint:

  * RunJsWorkflow         — durable wrapper around one ad-hoc run_js call.
  * ScheduledRunJsWorkflow — fired by Temporal Schedules; recovers its
                             config from the workflow's memo (the Temporal
                             schedule API in this SDK does not pass workflow
                             arguments through, so we stash the input on
                             the workflow's memo when the schedule is
                             created).

The REST API is the small surface the Rust MCP server talks to:

  POST   /workflows/run_js
  POST   /schedules
  GET    /schedules?session=<name>
  DELETE /schedules/<session>/<schedule_id>
  POST   /schedules/<session>/<schedule_id>/pause
  POST   /schedules/<session>/<schedule_id>/unpause
  POST   /schedules/<session>/<schedule_id>/trigger
  GET    /health
"""

from __future__ import annotations

import asyncio
import json
import logging
import os
import uuid
from contextlib import asynccontextmanager
from dataclasses import dataclass
from datetime import timedelta
from typing import Any, Optional

import httpx
import uvicorn
from fastapi import FastAPI, HTTPException, Query, Request
from temporalio import activity, workflow
from temporalio.client import (
    Client,
    Schedule,
    ScheduleActionStartWorkflow,
    ScheduleAlreadyRunningError,
    ScheduleIntervalSpec,
    ScheduleSpec,
    ScheduleState,
)
from temporalio.common import RawValue
from temporalio.worker import Worker

logging.basicConfig(level=os.environ.get("LOG_LEVEL", "INFO"))
log = logging.getLogger("mcp-temporal-worker")

TEMPORAL_ADDRESS = os.environ.get("TEMPORAL_ADDRESS", "temporal:7233")
TEMPORAL_NAMESPACE = os.environ.get("TEMPORAL_NAMESPACE", "default")
TASK_QUEUE = os.environ.get("TEMPORAL_TASK_QUEUE", "mcp-js")
MCP_SERVER_URL = os.environ.get("MCP_SERVER_URL", "http://node1:3000").rstrip("/")
HTTP_PORT = int(os.environ.get("HTTP_PORT", "8000"))

POLL_INTERVAL_SECS = 0.2

SCHEDULE_ID_PREFIX = "mcpjs"


def _server_schedule_id(session: str, schedule_id: str) -> str:
    return f"{SCHEDULE_ID_PREFIX}-{session}-{schedule_id}"


# ── Activity ──────────────────────────────────────────────────────────────


@dataclass
class RunJsInput:
    code: str
    heap: Optional[str] = None
    session: Optional[str] = None
    heap_memory_max_mb: Optional[int] = None
    execution_timeout_secs: Optional[int] = None
    tags: Optional[dict[str, str]] = None
    mcp_headers: Optional[dict[str, Any]] = None


@dataclass
class RunJsOutput:
    execution_id: str
    status: str
    result: Any = None
    heap: Optional[str] = None
    error: Optional[str] = None


@activity.defn
async def run_js_activity(input: RunJsInput) -> RunJsOutput:
    """Submit one run_js to the MCP-JS server and poll until it terminates."""
    payload: dict[str, Any] = {"code": input.code}
    for k in ("heap", "session", "heap_memory_max_mb", "execution_timeout_secs", "tags"):
        v = getattr(input, k)
        if v is not None:
            payload[k] = v

    timeout = httpx.Timeout(60.0, connect=10.0)
    async with httpx.AsyncClient(timeout=timeout) as client:
        resp = await client.post(f"{MCP_SERVER_URL}/api/exec", json=payload)
        resp.raise_for_status()
        execution_id = resp.json()["execution_id"]
        log.info("submitted execution %s", execution_id)

        while True:
            activity.heartbeat()
            await asyncio.sleep(POLL_INTERVAL_SECS)
            r = await client.get(f"{MCP_SERVER_URL}/api/executions/{execution_id}")
            if r.status_code == 404:
                continue
            r.raise_for_status()
            info = r.json()
            status = info.get("status", "")
            if status in ("completed", "failed", "cancelled", "timed_out"):
                return RunJsOutput(
                    execution_id=execution_id,
                    status=status,
                    result=info.get("result"),
                    heap=info.get("heap"),
                    error=info.get("error"),
                )


# ── Workflows ─────────────────────────────────────────────────────────────


@workflow.defn(name="RunJsWorkflow")
class RunJsWorkflow:
    @workflow.run
    async def run(self, input: RunJsInput) -> RunJsOutput:
        timeout = (input.execution_timeout_secs or 30) + 30
        return await workflow.execute_activity(
            run_js_activity,
            input,
            start_to_close_timeout=timedelta(seconds=timeout),
            heartbeat_timeout=timedelta(seconds=15),
        )


@workflow.defn(name="ScheduledRunJsWorkflow")
class ScheduledRunJsWorkflow:
    """Scheduled-task workflow.

    Schedules in Temporal cannot pass arbitrary input to the workflow, so we
    stash the `RunJsInput` on the workflow's memo when creating the schedule.
    The workflow reads it from `workflow.info().memo` and runs the activity.
    """

    @workflow.run
    async def run(self) -> RunJsOutput:
        memo = workflow.memo() or {}
        raw = memo.get("input")
        if raw is None:
            raise RuntimeError("scheduled workflow has no `input` memo")
        if isinstance(raw, RawValue):
            raw = workflow.payload_converter().from_payload(raw.payload, dict)
        input = RunJsInput(**raw)
        timeout = (input.execution_timeout_secs or 30) + 30
        return await workflow.execute_activity(
            run_js_activity,
            input,
            start_to_close_timeout=timedelta(seconds=timeout),
            heartbeat_timeout=timedelta(seconds=15),
        )


# ── REST control plane ────────────────────────────────────────────────────


_client: Optional[Client] = None
_worker_task: Optional[asyncio.Task] = None


async def get_client() -> Client:
    if _client is None:
        raise RuntimeError("Temporal client not initialized")
    return _client


@asynccontextmanager
async def lifespan(_app: FastAPI):
    global _client, _worker_task
    log.info("connecting to Temporal at %s (namespace=%s)", TEMPORAL_ADDRESS, TEMPORAL_NAMESPACE)
    _client = await Client.connect(TEMPORAL_ADDRESS, namespace=TEMPORAL_NAMESPACE)
    worker = Worker(
        _client,
        task_queue=TASK_QUEUE,
        workflows=[RunJsWorkflow, ScheduledRunJsWorkflow],
        activities=[run_js_activity],
    )
    log.info("starting Temporal worker on task queue %s", TASK_QUEUE)
    _worker_task = asyncio.create_task(worker.run())
    try:
        yield
    finally:
        if _worker_task is not None:
            _worker_task.cancel()
            try:
                await _worker_task
            except (asyncio.CancelledError, Exception):
                pass


app = FastAPI(lifespan=lifespan)


@app.get("/health")
async def health() -> dict[str, str]:
    return {"status": "ok", "task_queue": TASK_QUEUE}


@app.post("/workflows/run_js")
async def submit_run_js(req: Request) -> dict[str, str]:
    body = await req.json()
    input = RunJsInput(**body)
    workflow_id = f"mcpjs-run-{uuid.uuid4()}"
    client = await get_client()
    handle = await client.start_workflow(
        RunJsWorkflow.run,
        input,
        id=workflow_id,
        task_queue=TASK_QUEUE,
    )
    return {"workflow_id": handle.id, "run_id": handle.first_execution_run_id or ""}


@app.post("/schedules")
async def create_schedule(req: Request) -> dict[str, Any]:
    body = await req.json()
    session = body["session"]
    schedule_id = body["schedule_id"]
    cron = body.get("cron")
    every_secs = body.get("every_secs")
    if cron is None and every_secs is None:
        raise HTTPException(400, "either `cron` or `every_secs` is required")
    raw_input = body["input"]
    note = body.get("note")
    paused = bool(body.get("paused", False))

    sid = _server_schedule_id(session, schedule_id)
    client = await get_client()

    if cron is not None:
        spec = ScheduleSpec(cron_expressions=[cron])
    else:
        spec = ScheduleSpec(intervals=[ScheduleIntervalSpec(every=timedelta(seconds=int(every_secs)))])

    action = ScheduleActionStartWorkflow(
        "ScheduledRunJsWorkflow",
        id=f"{sid}-wf",
        task_queue=TASK_QUEUE,
        memo={"input": raw_input, "session": session, "schedule_id": schedule_id},
    )

    schedule = Schedule(action=action, spec=spec, state=ScheduleState(paused=paused, note=note or ""))

    try:
        handle = await client.create_schedule(sid, schedule)
    except ScheduleAlreadyRunningError:
        raise HTTPException(409, f"schedule {schedule_id!r} already exists for session {session!r}")
    return {
        "schedule_id": schedule_id,
        "server_schedule_id": handle.id,
        "session": session,
        "cron": cron,
        "every_secs": every_secs,
        "paused": paused,
    }


@app.get("/schedules")
async def list_schedules(session: str = Query(...)) -> dict[str, Any]:
    client = await get_client()
    prefix = _server_schedule_id(session, "")
    out = []
    async for entry in await client.list_schedules():
        if not entry.id.startswith(prefix):
            continue
        user_schedule_id = entry.id[len(prefix):]
        out.append(
            {
                "schedule_id": user_schedule_id,
                "server_schedule_id": entry.id,
                "session": session,
                "paused": (entry.info.paused if entry.info else False),
                "note": getattr(getattr(entry, "state", None), "note", None),
            }
        )
    return {"schedules": out}


@app.delete("/schedules/{session}/{schedule_id}")
async def delete_schedule(session: str, schedule_id: str) -> dict[str, bool]:
    client = await get_client()
    handle = client.get_schedule_handle(_server_schedule_id(session, schedule_id))
    await handle.delete()
    return {"ok": True}


async def _pause_or_unpause(session: str, schedule_id: str, action: str, body: dict[str, Any]) -> dict[str, bool]:
    client = await get_client()
    handle = client.get_schedule_handle(_server_schedule_id(session, schedule_id))
    note = body.get("note") if isinstance(body, dict) else None
    if action == "pause":
        await handle.pause(note=note)
    else:
        await handle.unpause(note=note)
    return {"ok": True}


@app.post("/schedules/{session}/{schedule_id}/pause")
async def pause_schedule(session: str, schedule_id: str, req: Request) -> dict[str, bool]:
    body = {}
    try:
        body = await req.json()
    except Exception:
        pass
    return await _pause_or_unpause(session, schedule_id, "pause", body or {})


@app.post("/schedules/{session}/{schedule_id}/unpause")
async def unpause_schedule(session: str, schedule_id: str, req: Request) -> dict[str, bool]:
    body = {}
    try:
        body = await req.json()
    except Exception:
        pass
    return await _pause_or_unpause(session, schedule_id, "unpause", body or {})


@app.post("/schedules/{session}/{schedule_id}/trigger")
async def trigger_schedule(session: str, schedule_id: str) -> dict[str, bool]:
    client = await get_client()
    handle = client.get_schedule_handle(_server_schedule_id(session, schedule_id))
    await handle.trigger()
    return {"ok": True}


def main() -> None:
    uvicorn.run("worker:app", host="0.0.0.0", port=HTTP_PORT, log_level="info")


if __name__ == "__main__":
    main()
