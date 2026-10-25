# Scheduler and HTTP feature status

These pages describe the experimental scheduler and dynamic HTTP functions with
[PR #284 hardening at `ffe1195`](https://github.com/r33drichards/mcp-js/commit/ffe1195c81290eb03c4576d643be34a584c1bfe1)
integrated through #287, #293, and #295. They do not imply that the feature stack
has merged into `main`, been deployed, or passed fresh combined-stack validation.

## Integrated runtime boundary

The stack is `main → #281 → #282 → #284 → #287 → #293 → #295`.
Earlier documentation of #293 at `5c52682` described the pre-hardening runtime;
those historical test results are not validation of this integrated revision.

| Concern | Integrated behavior |
|---|---|
| Scheduler Raft transport | Required `cluster_bearer_env`; all fixed-voter HTTP routes authenticate the shared bearer and bind the configured advertised address |
| Resource bounds | 4 MiB checkpoint cap; claim/admission metadata retires with core records; 512 KiB scheduled/admission bodies; bounded replication batches and checked terms; no general log compaction |
| Admission refusal | HTTP 409 and definite connection-establishment errors advance to the next callback; ambiguous sends stop |
| Rejected admission | Only a bound, quorum-confirmed terminal fence can establish not-launched; existing permits and uncertain outcomes retain capacity |
| Uncertain cancellation | Explicit dispatcher-and-work quiescence attestation can abandon `Unknown` or unresolved `Cancelling`; never automatic |
| Shutdown and Engine lifetime | Admission remains available during drain; driver panic/abort does not confirm stopped while blocking execution remains alive |

Set `cluster_bearer_env` on every runner, resolving to the same existing secret on
all voters. Bearer authentication does not encrypt HTTP or make Raft Byzantine-
tolerant: keep private network boundaries and encrypted transport for remote links.
Legacy dynamic cluster mode is not changed by this fixed-voter protection.

## Implemented and excluded scope

Implemented here: numeric five-field cron with IANA timezones, missed-run,
overlap and retry policies, durable fixed-voter runner admission, dedicated
scheduled workers, explicit cancellation, dynamic HTTP definition CRUD and
invocation, conditional revisions, and NixOS integration tests.

The design documents also discuss future work. There is no scheduling MCP
management tool, MCP Events delivery, multi-tenant principal-to-grant binding,
exactly-once external effects, automatic resolution of `Unknown`, schema
migration, or production-readiness guarantee. Dynamic HTTP handlers have no
per-definition owner/role model, wildcard routes, streaming, or upgrades.

## Choose a page

| Need | Documentation |
|---|---|
| Learn with a small working example | [Schedule a first run](../tutorials/first-schedule.md), [Create an HTTP function](../tutorials/first-http-function.md) |
| Operate or deploy | [Stop scheduled work](../how-to/stop-scheduled-work.md), [Update HTTP handlers](../how-to/manage-http-functions.md), [Deploy and test on NixOS](../how-to/scheduler-http-nixos.md) |
| Look up fields and limits | [Scheduler reference](scheduler.md), [HTTP functions reference](http-functions.md) |
| Understand the safety model | [Admission and cancellation](../concepts/scheduler-admission.md), [HTTP revisions and execution](../concepts/http-functions.md) |

The site follows [Diátaxis](https://diataxis.fr/): tutorials teach through guided
practice; how-to guides solve operational tasks; reference records contracts;
explanation develops the model. The existing **Concepts** section contains
explanation. The repository READMEs and design plans remain supporting material.
