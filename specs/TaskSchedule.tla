---------------------------- MODULE TaskSchedule ----------------------------
(***************************************************************************)
(* DESIGN MODEL ONLY: a scheduling layer in front of existing MCP Tasks.  *)
(* A schedule creates a task immediately when its occurrence is observed. *)
(* Task execution is abstracted to active -> terminal; no worker retries,  *)
(* leases, snapshots, outbox, persistence, or crash recovery are modelled. *)
(*                                                                         *)
(* Periodic cadence is fixed: FirstDue + n * Interval. A clock observation *)
(* may jump forward; periodic slots strictly before it are skipped. At    *)
(* an observed due time, an active task causes that slot to be skipped.   *)
(* Disable prevents new tasks, but does not terminate an existing task.   *)
(*                                                                         *)
(* Overdue one-time behavior is UNDECIDED. Advance cannot jump across a   *)
(* pending one-time deadline. This is a scope restriction, not a proposed *)
(* runtime restriction. No clock or process restart semantics are claimed.*)
(*                                                                         *)
(* Three constants inject faulty designs for regression counterexamples: *)
(* AllowOverlap, KeepMissed, ConsumeOccurrence. Production design values  *)
(* are FALSE, FALSE, TRUE; these are NOT exposed scheduling policies.     *)
(***************************************************************************)
\* Reading guide for TLA+ newcomers:
\*   /\ and \/ mean AND and OR; ~ means NOT; => means implies.
\*   \A / \E mean "for every" / "there exists"; \in means membership.
\*   x is the current value; x' is the value after one atomic action.
\*   [f EXCEPT ![s] = ...] updates one entry in a function/map.
\*   @ inside EXCEPT is the old value at the entry being updated.
\*   UNCHANGED lists everything that the action leaves alone.
\*   <<...>> is a sequence/tuple; |-> constructs a record or function.
\* TLC explores all enabled action orders, not one fixed execution trace.
EXTENDS Naturals, Sequences, FiniteSets

\* Fixed inputs supplied by a TLC config:
\*   Schedules: schedule IDs, not task IDs or workers.
\*   FirstDue[s]: first occurrence's logical timestamp.
\*   Interval[s]: zero for one-time; positive for a repeating schedule.
\*   MaxTime: finite clock horizon that keeps model checking tractable.
\*   MaxTasks: bound on the audit history, NOT a runtime queue capacity.
\* The remaining constants deliberately break admission rules in negative
\* configs. The safe config sets FALSE, FALSE, TRUE respectively.
CONSTANTS Schedules, FirstDue, Interval, MaxTime, MaxTasks,
          AllowOverlap, KeepMissed, ConsumeOccurrence

\* These are constraints on inputs, not properties of an implementation.
\* [Schedules -> 1..MaxTime] means a total function from IDs to timestamps.
ASSUME /\ Schedules # {}
       /\ FirstDue \in [Schedules -> 1..MaxTime]
       /\ Interval \in [Schedules -> 0..MaxTime]
       /\ MaxTasks > 0
       /\ AllowOverlap \in BOOLEAN /\ KeepMissed \in BOOLEAN
       /\ ConsumeOccurrence \in BOOLEAN

\* Small TLC fixture: one one-time schedule and one periodic schedule.
ModelFirstDue == [s \in Schedules |-> IF s = "once" THEN 2 ELSE 1]
ModelInterval == [s \in Schedules |-> IF s = "once" THEN 0 ELSE 2]

\* Mutable state, indexed by schedule ID where appropriate:
\*   now: current scheduler observation, not continuous wall-clock time.
\*   created[s]: whether this schedule has been registered (never resets).
\*   enabled[s]: whether new occurrences may be admitted.
\*   nextDue[s]: earliest occurrence not yet fired or skipped.
\*   running[s]: number of active executions for this schedule. A count,
\*               rather than a boolean, lets TLC detect accidental overlap.
\*   fires: append-only audit of admissions, including the due slot, the
\*          observed time, and the enabled flag at the moment of admission.
\* Results, tool payloads, and MCP task status details do not affect these
\* scheduling rules, so they are omitted from the state space.
VARIABLES now, created, enabled, nextDue, running, fires
\* The complete state tuple is used for stuttering and fairness below.
vars == <<now, created, enabled, nextDue, running, fires>>
\* A sentinel beyond every timestamp that can be produced by one advance:
\* due <= MaxTime and interval <= MaxTime, so due + interval <= 2*MaxTime.
\* Uncreated and consumed one-time schedules use NoDue and cannot fire.
NoDue == 2 * MaxTime + 1
\* Consuming a periodic slot preserves its cadence. Consuming a one-time
\* slot removes its only deadline; finishing the task does not re-arm it.
NextSlot(s, d) == IF Interval[s] = 0 THEN NoDue ELSE d + Interval[s]
\* Logical occurrence identity is (schedule, original due time). Different
\* periodic fires share a schedule ID but have different occurrence IDs.
Occurrences == {<<s, d>> : s \in Schedules, d \in 1..MaxTime}
\* Due deliberately includes past slots (<=), not just exact equality.
\* The safe clock action removes missed periodic slots before admission.
\* Keeping past slots in a faulty config makes catch-up observable in TLC.
Due(s) == created[s] /\ nextDue[s] <= now

\* Begin before any deadlines, with no schedules and no executions.
\* TLC can then interleave Create, Disable, and time observations freely.
Init ==
    /\ now = 0
    /\ created = [s \in Schedules |-> FALSE]
    /\ enabled = [s \in Schedules |-> FALSE]
    /\ nextDue = [s \in Schedules |-> NoDue]
    /\ running = [s \in Schedules |-> 0]
    /\ fires = <<>>

\* Tool name/arguments are immutable payloads, irrelevant to these properties.
\* Registration is allowed once per ID and only before/at its first due
\* time. Registering an already-overdue schedule is outside this model.
\* "created" stays true after Disable: this slice has no re-enable/update.
Create(s) ==
    /\ ~created[s] /\ now <= FirstDue[s]
    /\ created' = [created EXCEPT ![s] = TRUE]
    /\ enabled' = [enabled EXCEPT ![s] = TRUE]
    /\ nextDue' = [nextDue EXCEPT ![s] = FirstDue[s]]
    /\ UNCHANGED <<now, running, fires>>

\* Disable only admission. An already-running task is still running and
\* can finish normally; disabling does not rewrite the historical audit.
\* Disabled due slots are consumed by Skip so they cannot block the clock.
Disable(s) ==
    /\ created[s] /\ enabled[s]
    /\ enabled' = [enabled EXCEPT ![s] = FALSE]
    /\ UNCHANGED <<now, created, nextDue, running, fires>>

\* Task creation and consuming the occurrence are one scheduler action.
\* This is local admission, not a distributed exactly-once protocol.
\* All guards must hold in the same state: the slot is due, the schedule
\* is enabled, and (in the safe design) it has no active execution.
\* The audit bound is high enough for every eligible safe occurrence in
\* the committed fixture; otherwise it could artificially block progress.
\* Recording the admission, occupying the active slot, and advancing the
\* deadline happen atomically. There is no create/consume crash gap here.
Fire(s) ==
    /\ Due(s) /\ enabled[s]
    /\ (AllowOverlap \/ running[s] = 0)
    /\ Len(fires) < MaxTasks
    /\ fires' = Append(fires, [schedule |-> s, due |-> nextDue[s],
                               at |-> now, allowed |-> enabled[s]])
    /\ running' = [running EXCEPT ![s] = @ + 1]
    \* A missing consume step allows Finish followed by another Fire for
    \* the SAME deadline. The duplicate config exercises that exact bug.
    /\ nextDue' = IF ConsumeOccurrence
                  THEN [nextDue EXCEPT ![s] = NextSlot(s, @)] ELSE nextDue
    /\ UNCHANGED <<now, created, enabled>>

\* Never queue an overlapping or disabled occurrence for later execution.
\* This decides a currently due occurrence without creating a task.
\* If the previous execution outlasts its next deadline, that occurrence
\* is lost rather than queued. Finish later only releases the active slot.
\* Fire and Skip can race with Finish: if Finish happens first, admission
\* is possible; if Skip happens first, that occurrence remains skipped.
Skip(s) ==
    /\ Due(s)
    /\ (~enabled[s] \/ (~AllowOverlap /\ running[s] > 0))
    /\ nextDue' = [nextDue EXCEPT ![s] = NextSlot(s, @)]
    /\ UNCHANGED <<now, created, enabled, running, fires>>

\* Terminal here means underlying execution has ended. A cooperative cancel
\* acknowledgement alone must not free the non-overlap slot.
\* Success/failure are identical for admission purposes once execution
\* has actually stopped. No result inspection or automatic retry is added.
\* Disabled schedules can still finish their existing executions.
Finish(s) ==
    /\ running[s] > 0
    /\ running' = [running EXCEPT ![s] = @ - 1]
    /\ UNCHANGED <<now, created, enabled, nextDue, fires>>

\* Preserve the original cadence, choosing the first slot >= observed time.
\* Round UP along the original cadence, rather than restarting at t:
\*   old deadline 1, interval 2, observation 4 -> next deadline 5.
\*   old deadline 1, interval 2, observation 3 -> deadline 3 is eligible.
\* For positive gap g = t - nextDue, (g-1) div interval + 1 is ceil(g/interval).
\* The strict nextDue < t guard avoids negative subtraction and preserves
\* a slot exactly at t. Uncreated/one-time schedules are left untouched.
ObservedDue(s, t) ==
    IF created[s] /\ Interval[s] > 0 /\ nextDue[s] < t /\ ~KeepMissed
    THEN nextDue[s] + (((t - nextDue[s] - 1) \div Interval[s]) + 1) * Interval[s]
    ELSE nextDue[s]

\* Choose ANY later observation within the finite horizon. This explores
\* ordinary ticks as well as gaps that omit multiple periodic occurrences.
\* Finish/Disable may interleave with all these clock choices.
\* Process the previous observation's due decisions before moving on:
\* otherwise clock advancement alone could starve an eligible Fire/Skip.
\* This batching abstraction is not a promise of zero real timer latency.
Advance(t) ==
    /\ t \in (now + 1)..MaxTime
    /\ \A s \in Schedules : ~Due(s)
    \* Do not decide how overdue one-time schedules behave in this slice.
    /\ \A s \in Schedules :
           created[s] /\ enabled[s] /\ Interval[s] = 0 => nextDue[s] >= t
    /\ now' = t
    /\ nextDue' = [s \in Schedules |-> ObservedDue(s, t)]
    /\ UNCHANGED <<created, enabled, running, fires>>

\* One step chooses one enabled action for one schedule, or advances time.
\* The existential choices let TLC explore competing interleavings. This
\* does not assign tasks to workers or add multi-node scheduler ownership.
Next ==
    \/ \E s \in Schedules : Create(s) \/ Disable(s) \/ Fire(s) \/ Skip(s) \/ Finish(s)
    \/ \E t \in 1..MaxTime : Advance(t)

\* Weak fairness (WF): an action that remains continuously enabled cannot
\* be ignored forever. Without it, an execution could stutter forever and
\* never Finish, making any eventual-progress property false by design.
\* Finish fairness assumes execution terminates; arbitrary JS/MCP Tasks do
\* not guarantee that. Create/Disable need not happen, and any particular
\* clock destination need not be chosen; only clock progress is fair.
Fairness ==
    /\ \A s \in Schedules : WF_vars(Fire(s)) /\ WF_vars(Skip(s))
                             /\ WF_vars(Finish(s))
    /\ WF_vars(\E t \in 1..MaxTime : Advance(t))
\* [] means "always". [Next]_vars permits Next OR a stuttering step that
\* leaves the whole state unchanged. Stuttering permits finite-horizon
\* terminal states; fairness still excludes ignoring enabled fair actions.
Spec == Init /\ [][Next]_vars /\ Fairness

\* Safety properties describe every reachable state. This first invariant
\* catches invalid ranges or malformed audit entries, independently of the
\* more specific scheduling rules below.
TypeOK ==
    /\ now \in 0..MaxTime
    /\ created \in [Schedules -> BOOLEAN]
    /\ enabled \in [Schedules -> BOOLEAN]
    /\ nextDue \in [Schedules -> 1..NoDue]
    /\ running \in [Schedules -> 0..MaxTasks]
    /\ Len(fires) <= MaxTasks
    /\ \A i \in 1..Len(fires) :
           /\ fires[i].schedule \in Schedules
           /\ fires[i].due \in 1..MaxTime
           /\ fires[i].at \in 0..MaxTime
           /\ fires[i].allowed \in BOOLEAN

\* Admission may not precede the occurrence's scheduled timestamp.
NoEarlyFire == \A i \in 1..Len(fires) : fires[i].at >= fires[i].due
\* In this idealized observation model, only the observed slot fires.
\* A past deadline fired after a jump witnesses an unwanted catch-up run.
\* Equality is not a real-world wall-clock dispatch precision requirement.
NoCatchUp == \A i \in 1..Len(fires) : fires[i].at = fires[i].due
\* Different schedules may execute concurrently; only the SAME schedule
\* is serialized. The overlap fault makes this count reach two.
NoOverlap == \A s \in Schedules : running[s] <= 1
\* Check the enabled flag CAPTURED at admission, not its current value:
\* disabling a schedule later does not invalidate its earlier admissions.
NoDisabledFire == \A i \in 1..Len(fires) : fires[i].allowed
\* Count admissions for each logical occurrence in the immutable audit.
\* Finishing a task must never make the same occurrence eligible again.
NoDuplicateFire ==
    \A o \in Occurrences :
        Cardinality({i \in 1..Len(fires) :
                     <<fires[i].schedule, fires[i].due>> = o}) <= 1
\* Stronger per-schedule bound for one-time schedules: they have only one
\* occurrence. Periodic schedules intentionally admit more than one task.
OneTimeAtMostOnce ==
    \A s \in Schedules : Interval[s] = 0 =>
        Cardinality({i \in 1..Len(fires) : fires[i].schedule = s}) <= 1
\* Every future periodic deadline differs from its original first deadline
\* by an exact multiple of the interval, even after skipped occurrences.
\* % means modulo. This catches drift to "completion time + interval".
CadencePreserved ==
    \A s \in Schedules : created[s] /\ Interval[s] > 0 =>
        (nextDue[s] - FirstDue[s]) % Interval[s] = 0

\* Conditional progress: observed slots are decided; assumed terminating
\* executions release their slots. Neither property asserts crash recovery.
\* ~> means "leads to": whenever a slot is due, eventually it is no longer
\* due. Either Fire OR Skip satisfies this; successful execution is not
\* required. The occurrence must not remain stuck in scheduler bookkeeping.
DueEventuallyProcessed ==
    \A s \in Schedules : Due(s) ~> ~Due(s)
\* If an execution is active, its schedule eventually becomes idle. This
\* relies on Finish fairness and is separate from admitting future slots.
TasksEventuallyFinish ==
    \A s \in Schedules : (running[s] > 0) ~> (running[s] = 0)
=============================================================================
