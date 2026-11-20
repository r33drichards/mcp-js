# Scheduler and HTTP feature status

The experimental scheduler, dynamic HTTP functions, and HTTP/MCP management
layer are merged into `main`. The contracts below describe the implemented
runtime, including fenced worker admission and explicit recovery. Historical
PR validation records remain historical; release validation must run on the
revision being shipped.

## Runtime boundary

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

## Implemented features and remaining work

Implemented here: numeric five-field cron with IANA timezones, missed-run,
overlap and retry policies, durable fixed-voter runner admission, dedicated
scheduled workers, explicit cancellation, dynamic HTTP definition CRUD and
invocation, conditional revisions, and NixOS integration tests. The management
layer adds dedicated HTTP-function MCP tools and opt-in scheduler HTTP/MCP
operator access. Both transports reuse the existing operations and state model.

[Scheduler production readiness](https://github.com/r33drichards/mcp-js/issues/348)
tracks recovery workflows, compaction, upgrades, observability, and soak testing.
The design documents also discuss future work. There is no MCP Events delivery,
multi-tenant principal-to-grant binding,
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

## Source

- [Runner](https://github.com/r33drichards/mcp-js/blob/main/scheduler-runner/src/replicated.rs)
- [Worker](https://github.com/r33drichards/mcp-js/blob/main/server/src/scheduled_worker.rs)
- [HTTP registry](https://github.com/r33drichards/mcp-js/blob/main/server/src/http_functions.rs)
- [Management adapters](https://github.com/r33drichards/mcp-js/blob/main/scheduler-runner/src/management.rs)
