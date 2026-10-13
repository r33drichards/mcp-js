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
EXTENDS Naturals, Sequences, FiniteSets

CONSTANTS Schedules, FirstDue, Interval, MaxTime, MaxTasks,
          AllowOverlap, KeepMissed, ConsumeOccurrence

ASSUME /\ Schedules # {}
       /\ FirstDue \in [Schedules -> 1..MaxTime]
       /\ Interval \in [Schedules -> 0..MaxTime]
       /\ MaxTasks > 0
       /\ AllowOverlap \in BOOLEAN /\ KeepMissed \in BOOLEAN
       /\ ConsumeOccurrence \in BOOLEAN

\* Small TLC fixture: one one-time schedule and one periodic schedule.
ModelFirstDue == [s \in Schedules |-> IF s = "once" THEN 2 ELSE 1]
ModelInterval == [s \in Schedules |-> IF s = "once" THEN 0 ELSE 2]

VARIABLES now, created, enabled, nextDue, running, fires
vars == <<now, created, enabled, nextDue, running, fires>>
NoDue == 2 * MaxTime + 1
NextSlot(s, d) == IF Interval[s] = 0 THEN NoDue ELSE d + Interval[s]
Occurrences == {<<s, d>> : s \in Schedules, d \in 1..MaxTime}
Due(s) == created[s] /\ nextDue[s] <= now

Init ==
    /\ now = 0
    /\ created = [s \in Schedules |-> FALSE]
    /\ enabled = [s \in Schedules |-> FALSE]
    /\ nextDue = [s \in Schedules |-> NoDue]
    /\ running = [s \in Schedules |-> 0]
    /\ fires = <<>>

\* Tool name/arguments are immutable payloads, irrelevant to these properties.
Create(s) ==
    /\ ~created[s] /\ now <= FirstDue[s]
    /\ created' = [created EXCEPT ![s] = TRUE]
    /\ enabled' = [enabled EXCEPT ![s] = TRUE]
    /\ nextDue' = [nextDue EXCEPT ![s] = FirstDue[s]]
    /\ UNCHANGED <<now, running, fires>>

Disable(s) ==
    /\ created[s] /\ enabled[s]
    /\ enabled' = [enabled EXCEPT ![s] = FALSE]
    /\ UNCHANGED <<now, created, nextDue, running, fires>>

\* Task creation and consuming the occurrence are one scheduler action.
\* This is local admission, not a distributed exactly-once protocol.
Fire(s) ==
    /\ Due(s) /\ enabled[s]
    /\ (AllowOverlap \/ running[s] = 0)
    /\ Len(fires) < MaxTasks
    /\ fires' = Append(fires, [schedule |-> s, due |-> nextDue[s],
                               at |-> now, allowed |-> enabled[s]])
    /\ running' = [running EXCEPT ![s] = @ + 1]
    /\ nextDue' = IF ConsumeOccurrence
                  THEN [nextDue EXCEPT ![s] = NextSlot(s, @)] ELSE nextDue
    /\ UNCHANGED <<now, created, enabled>>

\* Never queue an overlapping or disabled occurrence for later execution.
Skip(s) ==
    /\ Due(s)
    /\ (~enabled[s] \/ (~AllowOverlap /\ running[s] > 0))
    /\ nextDue' = [nextDue EXCEPT ![s] = NextSlot(s, @)]
    /\ UNCHANGED <<now, created, enabled, running, fires>>

\* Terminal here means underlying execution has ended. A cooperative cancel
\* acknowledgement alone must not free the non-overlap slot.
Finish(s) ==
    /\ running[s] > 0
    /\ running' = [running EXCEPT ![s] = @ - 1]
    /\ UNCHANGED <<now, created, enabled, nextDue, fires>>

\* Preserve the original cadence, choosing the first slot >= observed time.
ObservedDue(s, t) ==
    IF created[s] /\ Interval[s] > 0 /\ nextDue[s] < t /\ ~KeepMissed
    THEN nextDue[s] + (((t - nextDue[s] - 1) \div Interval[s]) + 1) * Interval[s]
    ELSE nextDue[s]

Advance(t) ==
    /\ t \in (now + 1)..MaxTime
    /\ \A s \in Schedules : ~Due(s)
    \* Do not decide how overdue one-time schedules behave in this slice.
    /\ \A s \in Schedules :
           created[s] /\ enabled[s] /\ Interval[s] = 0 => nextDue[s] >= t
    /\ now' = t
    /\ nextDue' = [s \in Schedules |-> ObservedDue(s, t)]
    /\ UNCHANGED <<created, enabled, running, fires>>

Next ==
    \/ \E s \in Schedules : Create(s) \/ Disable(s) \/ Fire(s) \/ Skip(s) \/ Finish(s)
    \/ \E t \in 1..MaxTime : Advance(t)

Fairness ==
    /\ \A s \in Schedules : WF_vars(Fire(s)) /\ WF_vars(Skip(s))
                             /\ WF_vars(Finish(s))
    /\ WF_vars(\E t \in 1..MaxTime : Advance(t))
Spec == Init /\ [][Next]_vars /\ Fairness

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

NoEarlyFire == \A i \in 1..Len(fires) : fires[i].at >= fires[i].due
NoCatchUp == \A i \in 1..Len(fires) : fires[i].at = fires[i].due
NoOverlap == \A s \in Schedules : running[s] <= 1
NoDisabledFire == \A i \in 1..Len(fires) : fires[i].allowed
NoDuplicateFire ==
    \A o \in Occurrences :
        Cardinality({i \in 1..Len(fires) :
                     <<fires[i].schedule, fires[i].due>> = o}) <= 1
OneTimeAtMostOnce ==
    \A s \in Schedules : Interval[s] = 0 =>
        Cardinality({i \in 1..Len(fires) : fires[i].schedule = s}) <= 1
CadencePreserved ==
    \A s \in Schedules : created[s] /\ Interval[s] > 0 =>
        (nextDue[s] - FirstDue[s]) % Interval[s] = 0

\* Conditional progress: observed slots are decided; assumed terminating
\* executions release their slots. Neither property asserts crash recovery.
DueEventuallyProcessed ==
    \A s \in Schedules : Due(s) ~> ~Due(s)
TasksEventuallyFinish ==
    \A s \in Schedules : (running[s] > 0) ~> (running[s] = 0)
=============================================================================
