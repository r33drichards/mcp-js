# Scheduler and HTTP feature status

These pages describe the HTTP/MCP management layer stacked on
[PR #295, commit `1eb74af`](https://github.com/r33drichards/mcp-js/commit/1eb74af4ace275bc803faf31d86c459433578495),
which documents the runtime inherited from
[PR #293, commit `5c52682`](https://github.com/r33drichards/mcp-js/commit/5c52682da519f0ced3035b2cab32b41868c9eb50).
They do not imply that the feature stack has merged into `main`.

## Integration prerequisite

The stack is `main → #281 → #282 → #284 → #287 → #293 → #295 → management layer`.
[PR #284](https://github.com/r33drichards/mcp-js/pull/284) has since advanced to
[`ffe1195`](https://github.com/r33drichards/mcp-js/commit/ffe1195c81290eb03c4576d643be34a584c1bfe1).
Its changes are **not integrated into this documentation's #293 base**.
Integrating and validating those fixes through #287 and #293 is a prerequisite
for treating the full stack as current. This documentation does not update those
parent branches.

| Concern | Observed in #293 | Pending upstream in #284 |
|---|---|---|
| Scheduler Raft transport | Fixed voters on a trusted network; RPCs lack authentication | Required cluster bearer reference, authenticated fixed-voter RPCs and listener binding |
| Resource bounds | Policy limits bound core records; retained claims/admissions and Raft log grow | Checkpoint/retained-record limits, smaller request bodies, bounded replication batches and checked terms; no general log compaction |
| Admission refusal | Only a definitive HTTP 409 advances to the next endpoint; a connection error stops the attempt | Definite connection-establishment errors also advance; ambiguous sends still stop |
| Rejected admission | Conservative uncertain outcome retains capacity | Bound, quorum-confirmed not-launched response distinguishes definite rejection |
| Uncertain cancellation | `acknowledge_stopped` accepts only `Unknown`, not `Cancelling` | Explicit quiescence attestation can also abandon unresolved `Cancelling` |
| Shutdown and Engine lifetime | Existing cancellation/join tracking; conservative uncertainty | Admission remains available during drain; additional panic/abort lifetime tracking |

Do not add the upcoming `cluster_bearer_env` key to the #293 runner config:
its parser rejects unknown fields. Recheck configuration, recovery instructions,
and test results when integrating the newer scheduler commits.

## Implemented and excluded scope

Implemented here: numeric five-field cron with IANA timezones, missed-run,
overlap and retry policies, durable fixed-voter runner admission, dedicated
scheduled workers, explicit cancellation, dynamic HTTP definition CRUD and
invocation, conditional revisions, and NixOS integration tests. The management
layer adds dedicated HTTP-function MCP tools and opt-in scheduler HTTP/MCP
operator access. Both transports reuse the existing operations and state model.

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
