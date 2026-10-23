# Actual Jupyter UI verification — 2026-10-23

Environment: JupyterLab 4.4.10, ipykernel 6.30.1, ipywidgets 8.1.7, headless system Chromium, Python 3.12. Jupyter was authenticated and bound to loopback. Tests used a scratch notebook copy, not the deliverable. No paid inference, external provider traffic, live MCP, Docker actions or AWS operations occurred.

## Delivered behavior reproduced

Executed setup, canonical prompt, agent-loop definition and UI cells in JupyterLab, then clicked the actual Start/Stop widgets. A labeled offline asynchronous fixture substituted for live network execution.

- Another Python cell executed while the task awaited I/O: `OTHER CELL ['started'] False` (False = background task not done).
- Stop cancellation completed: `AFTER STOP ['started', 'cancelled'] True`.
- Found a real rerun bug: rerunning controls left the former task active but set the current task reference to None: `AFTER RERUN ['started', 'cancelled', 'started'] False None`.

## Fixed behavior retested

The new controls cancel and await the previous task before replacement, disable old controls, refuse a replacement on cleanup timeout, explicitly append traces without holding another cell's output-capture context, and expose running/cancelled/failed/stopped status.

The browser retest used the notebook's actual investigation_loop with an offline model/session fixture and delayed model response; the real SDK network adapters were not invoked.

- Other cell remained executable while the loop waited.
- Stop completed cancellation.
- Rerun cancelled the second task: `AFTER RERUN ['started', 'cancelled', 'started', 'cancelled'] True None`.
- Rerun reset approval and task state: `RERUN GUARDS PASS`.
- Start without approval created no task: `NO APPROVAL: NO TASK`.
- A separately labeled short completion fixture restored Start and disabled Stop: `COMPLETION CONTROLS PASS`.
- Screenshot visually checked: widgets, status and explicit fixture trace rendered in the notebook. Screenshot kept in scratch, not included as a live-demo deliverable.

## Meaning and limits

This establishes responsiveness while an asyncio task awaits I/O, not parallel CPU execution of two cells. A long synchronous cell can delay callbacks, cancellation and the investigation. Existing operator subprocess buttons are synchronous; do not run them alongside the investigation. No background-thread/process migration is needed for the tested async investigation path.

Cancellation does not roll back effects already completed by external tools. Live model/MCP timeout cleanup, streamed traces from actual providers, full Astronomy/dashboard/browser handoff and cluster failure actions remain untested in this UI run. No Marimo conversion was requested or performed.

## Bedrock revision retest

After the requested provider change, repeated the same actual JupyterLab browser checks against the Bedrock notebook. A delayed offline Converse fixture drove the notebook's investigation_loop; concurrency, Stop, rerun cleanup, approval reset, and completed-run controls passed again. No AWS request was made. A separate offline test validated both Converse requests (before and after toolResult) against botocore 1.43.117's real Converse input shape. Installed async client: aiobotocore 3.10.0. Model ID remains parameterized pending runtime verification of cheapest viable tool-capable choice; this report makes no pricing or availability claim.
