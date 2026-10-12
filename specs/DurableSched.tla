---------------------------- MODULE DurableSched ----------------------------
(***************************************************************************)
(* Abstract model of a durable job scheduler embedded in mcp-js.           *)
(*                                                                         *)
(* DESIGN MODEL: the scheduler is a proposal and has no implementation     *)
(* yet.  The model exists to pick the retry and conflict policies before   *)
(* any code is written.  See README.md for the TLC results.                *)
(*                                                                         *)
(* What is modelled                                                        *)
(*   - One session whose head is a heap snapshot.  A heap is modelled as   *)
(*     the sequence of events that produced it, so equal sequences stand   *)
(*     for equal content hashes.                                           *)
(*   - Jobs that fire once.  A job is Steps steps of JS; each step mutates *)
(*     the worker's local heap AND performs one external side effect       *)
(*     (fetch, mcp.callTool).  A cron job is a series of such fires.       *)
(*   - Workers that claim a job, run it, optionally checkpoint, and        *)
(*     propose Complete.                                                   *)
(*   - Interactive run_js calls on the same session (UserRun).             *)
(*                                                                         *)
(* What is abstracted away                                                 *)
(*   - Raft: every scheduler command (Claim, Checkpoint, Complete, Expire) *)
(*     is one atomic step on a single linearizable state machine.          *)
(*   - Time: there are no clocks.  A lease may expire at ANY moment        *)
(*     (Expire), so every safety result holds for arbitrary clock skew,    *)
(*     GC pauses and slow workers.  A worker whose lease expired keeps     *)
(*     running as a zombie until it is fenced.                             *)
(*   - UserRun is atomic (its own load/execute/commit is not interleaved). *)
(*                                                                         *)
(* The two policies under study                                            *)
(*   RetryBase  - which heap a retry starts from                           *)
(*     "pinned"      the head captured at the job's first claim            *)
(*     "head"        the session head at the time of each claim            *)
(*     "checkpoint"  the job's last durable checkpoint, else as "pinned"   *)
(*   OnConflict - what Complete does when the session head is no longer    *)
(*                the heap the attempt started from                        *)
(*     "reject"      fail the attempt, keep the base                       *)
(*     "rebase"      fail the attempt, drop base and checkpoint, so the    *)
(*                   next attempt starts from the current head             *)
(*     "clobber"     commit anyway (last writer wins)                      *)
(***************************************************************************)
EXTENDS Naturals, Sequences, FiniteSets

CONSTANTS
    Jobs,          \* set of job ids, as strings
    Workers,       \* set of worker ids
    Steps,         \* steps per job
    MaxAttempts,   \* attempts before a job is dead
    MaxUser,       \* bound on interactive runs
    RetryBase,     \* "pinned" | "head" | "checkpoint"
    OnConflict,    \* "reject" | "rebase" | "clobber"
    Faults         \* BOOLEAN: enable crashes and lease expiry

ASSUME /\ RetryBase  \in {"pinned", "head", "checkpoint"}
       /\ OnConflict \in {"reject", "rebase", "clobber"}
       /\ Faults \in BOOLEAN
       /\ "none" \notin Jobs /\ "user" \notin Jobs

VARIABLES
    head,    \* session head: the committed heap
    job,     \* replicated job table
    wk,      \* worker-local state (lost on crash, NOT replicated)
    fx,      \* fx[j][s] = how many times step s of job j hit the outside world
    users    \* number of interactive runs so far

vars == <<head, job, wk, fx, users>>

NoHeap == << <<"none", 0>> >>          \* "no heap recorded" marker
Idle   == [job |-> "none", epoch |-> 0, parent |-> <<>>, heap |-> <<>>]

Count(h, e)      == Cardinality({i \in 1..Len(h) : h[i] = e})
StepsDone(h, j)  == Cardinality({i \in 1..Len(h) : h[i][1] = j})
IsPrefix(a, b)   == Len(a) <= Len(b) /\ SubSeq(b, 1, Len(a)) = a

Init ==
    /\ head  = <<>>
    /\ job   = [j \in Jobs |-> [state |-> "scheduled", epoch |-> 0,
                                attempt |-> 0, base |-> NoHeap, ckpt |-> NoHeap]]
    /\ wk    = [w \in Workers |-> Idle]
    /\ fx    = [j \in Jobs |-> [s \in 1..Steps |-> 0]]
    /\ users = 0

\* An attempt ended without committing: retry, or give up.
Failed(rec) ==
    IF rec.attempt >= MaxAttempts
    THEN [rec EXCEPT !.state = "dead"]
    ELSE [rec EXCEPT !.state = "scheduled"]

(***************************************************************************)
(* Claim: bump the epoch (the fencing token) and fix the attempt's base.   *)
(* This is where RetryBase takes effect.                                   *)
(***************************************************************************)
Claim(w, j) ==
    /\ wk[w].job = "none"
    /\ job[j].state = "scheduled"
    /\ LET b == IF RetryBase = "head" \/ job[j].base = NoHeap
                THEN head ELSE job[j].base
           c == IF RetryBase = "head" THEN NoHeap ELSE job[j].ckpt
           e == job[j].epoch + 1
           start == IF RetryBase = "checkpoint" /\ c # NoHeap THEN c ELSE b
       IN /\ job' = [job EXCEPT ![j] = [state |-> "claimed", epoch |-> e,
                                        attempt |-> @.attempt + 1,
                                        base |-> b, ckpt |-> c]]
          /\ wk'  = [wk EXCEPT ![w] = [job |-> j, epoch |-> e,
                                       parent |-> b, heap |-> start]]
    /\ UNCHANGED <<head, fx, users>>

(***************************************************************************)
(* Step: purely local.  No replicated state is read, so a zombie can keep  *)
(* stepping; its side effects still reach the outside world.               *)
(***************************************************************************)
Step(w) ==
    LET j == wk[w].job
        s == StepsDone(wk[w].heap, j) + 1
    IN /\ j # "none"
       /\ s <= Steps
       /\ wk' = [wk EXCEPT ![w].heap = Append(@, <<j, s>>)]
       /\ fx' = [fx EXCEPT ![j][s] = @ + 1]
       /\ UNCHANGED <<head, job, users>>

(***************************************************************************)
(* Checkpoint: store partial progress under the job (never in the session  *)
(* head), fenced by epoch.                                                 *)
(***************************************************************************)
Checkpoint(w) ==
    LET j == wk[w].job IN
    /\ RetryBase = "checkpoint"
    /\ j # "none"
    /\ wk[w].heap # wk[w].parent
    /\ StepsDone(wk[w].heap, j) < Steps
    /\ IF job[j].state = "claimed" /\ job[j].epoch = wk[w].epoch
       THEN /\ job[j].ckpt # wk[w].heap
            /\ job' = [job EXCEPT ![j].ckpt = wk[w].heap]
            /\ wk' = wk
       ELSE /\ wk' = [wk EXCEPT ![w] = Idle]          \* fenced
            /\ job' = job
    /\ UNCHANGED <<head, fx, users>>

(***************************************************************************)
(* Complete: one atomic command that checks the fence, compare-and-swaps   *)
(* the session head and marks the job done.  This is where OnConflict      *)
(* takes effect.                                                           *)
(***************************************************************************)
Complete(w) ==
    LET j == wk[w].job IN
    /\ j # "none"
    /\ StepsDone(wk[w].heap, j) = Steps
    /\ wk' = [wk EXCEPT ![w] = Idle]
    /\ IF ~(job[j].state = "claimed" /\ job[j].epoch = wk[w].epoch)
       THEN UNCHANGED <<head, job>>                   \* fenced
       ELSE IF head = wk[w].parent \/ OnConflict = "clobber"
       THEN /\ head' = wk[w].heap
            /\ job'  = [job EXCEPT ![j].state = "done"]
       ELSE /\ head' = head
            /\ job'  = [job EXCEPT ![j] =
                          Failed(IF OnConflict = "rebase"
                                 THEN [@ EXCEPT !.base = NoHeap, !.ckpt = NoHeap]
                                 ELSE @)]
    /\ UNCHANGED <<fx, users>>

\* The lease ran out as far as the state machine is concerned.
Expire(j) ==
    /\ Faults
    /\ job[j].state = "claimed"
    /\ job' = [job EXCEPT ![j] = Failed(@)]
    /\ UNCHANGED <<head, wk, fx, users>>

\* The worker process dies and loses everything local.
Crash(w) ==
    /\ Faults
    /\ wk[w].job # "none"
    /\ wk' = [wk EXCEPT ![w] = Idle]
    /\ UNCHANGED <<head, job, fx, users>>

\* An interactive run_js on the same session advances the head.
UserRun ==
    /\ users < MaxUser
    /\ users' = users + 1
    /\ head'  = Append(head, <<"user", users + 1>>)
    /\ UNCHANGED <<job, wk, fx>>

Next ==
    \/ \E w \in Workers, j \in Jobs : Claim(w, j)
    \/ \E w \in Workers : Step(w) \/ Checkpoint(w) \/ Complete(w) \/ Crash(w)
    \/ \E j \in Jobs : Expire(j)
    \/ UserRun

\* Crash and UserRun get no fairness: they may or may not happen.
Fairness ==
    /\ \A w \in Workers : WF_vars(Step(w)) /\ WF_vars(Checkpoint(w))
                          /\ WF_vars(Complete(w))
    /\ \A w \in Workers, j \in Jobs : WF_vars(Claim(w, j))
    /\ \A j \in Jobs : WF_vars(Expire(j))

Spec == Init /\ [][Next]_vars /\ Fairness

-----------------------------------------------------------------------------
(* Properties that should hold under every policy *)

TypeOK ==
    /\ \A j \in Jobs :
          /\ job[j].state \in {"scheduled", "claimed", "done", "dead"}
          /\ job[j].attempt \in 0..MaxAttempts
          /\ job[j].epoch = job[j].attempt
    /\ \A w \in Workers : wk[w].job \in Jobs \cup {"none"}
    /\ users \in 0..MaxUser

\* Fencing: at most one worker holds the current epoch of a claimed job.
OneOwner ==
    \A j \in Jobs :
        Cardinality({w \in Workers : /\ wk[w].job = j
                                     /\ job[j].state = "claimed"
                                     /\ wk[w].epoch = job[j].epoch}) <= 1

\* No step of any job is in the session heap twice.
HeapAtMostOnce ==
    \A j \in Jobs, s \in 1..Steps : Count(head, <<j, s>>) <= 1

\* The session head never contains a half-finished job.
NoPartialJob ==
    \A j \in Jobs : StepsDone(head, j) \in {0, Steps}

\* External side effects are at-least-once, bounded by the attempt budget.
FxBounded ==
    \A j \in Jobs, s \in 1..Steps : fx[j][s] <= MaxAttempts

\* Every job ends up done or dead.
Terminates ==
    <>(\A j \in Jobs : job[j].state \in {"done", "dead"})

-----------------------------------------------------------------------------
(* Properties that depend on the policy *)

\* No committed write is ever discarded: the head only ever extends.
HeadOnlyGrows == [][IsPrefix(head, head')]_head

\* A job marked done has its effects in the session head.
DoneIsDurable ==
    \A j \in Jobs : job[j].state = "done" =>
        \A s \in 1..Steps : Count(head, <<j, s>>) = 1

\* With crashes and expiry switched off, does any job still die?
NoDead ==
    \A j \in Jobs : job[j].state # "dead"

\* Probe, expected to fail everywhere: side effects are NOT exactly-once.
FxAtMostOnce ==
    \A j \in Jobs, s \in 1..Steps : fx[j][s] <= 1
=============================================================================
