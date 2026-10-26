-------------------------- MODULE TaskScheduleCron ---------------------------
(* Selected design: dispatch each observed new occurrence independently.  *)
(* Overlap is allowed. A prior running/unknown/failed execution does not   *)
(* delay the schedule or trigger retries. Missed periodic slots are lost. *)
EXTENDS Naturals, Sequences, FiniteSets

CONSTANTS Schedules, FirstDue, Interval, MaxTime, MaxTasks,
          KeepMissed, ConsumeOccurrence
ASSUME /\ Schedules # {}
       /\ FirstDue \in [Schedules -> 1..MaxTime]
       /\ Interval \in [Schedules -> 0..MaxTime]
       /\ MaxTasks >= MaxTime * Cardinality(Schedules)
       /\ KeepMissed \in BOOLEAN /\ ConsumeOccurrence \in BOOLEAN

ModelFirstDue == [s \in Schedules |-> IF s = "once" THEN 2 ELSE 1]
ModelInterval == [s \in Schedules |-> IF s = "once" THEN 0 ELSE 2]
NoDue == 2 * MaxTime + 1
NextSlot(s, d) == IF Interval[s] = 0 THEN NoDue ELSE d + Interval[s]
Occurrences == {<<s, d>> : s \in Schedules, d \in 1..MaxTime}

VARIABLES now, created, enabled, nextDue, running, fires
vars == <<now, created, enabled, nextDue, running, fires>>
Due(s) == created[s] /\ nextDue[s] <= now

Init ==
    /\ now = 0
    /\ created = [s \in Schedules |-> FALSE]
    /\ enabled = [s \in Schedules |-> FALSE]
    /\ nextDue = [s \in Schedules |-> NoDue]
    /\ running = [s \in Schedules |-> 0]
    /\ fires = <<>>

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

(* Atomic logical dispatch + occurrence consumption, not a distributed    *)
(* exactly-once delivery protocol. Crucially, running is NOT a guard.     *)
Fire(s) ==
    /\ Due(s) /\ enabled[s] /\ Len(fires) < MaxTasks
    /\ fires' = Append(fires, [schedule |-> s, due |-> nextDue[s],
                               at |-> now, allowed |-> enabled[s]])
    /\ running' = [running EXCEPT ![s] = @ + 1]
    /\ nextDue' = IF ConsumeOccurrence
                  THEN [nextDue EXCEPT ![s] = NextSlot(s, @)] ELSE nextDue
    /\ UNCHANGED <<now, created, enabled>>

(* Only disabled occurrences are skipped here; running/unknown attempts  *)
(* never cause a skip. Clock jumps below drop missed periodic slots.     *)
SkipDisabled(s) ==
    /\ Due(s) /\ ~enabled[s]
    /\ nextDue' = [nextDue EXCEPT ![s] = NextSlot(s, @)]
    /\ UNCHANGED <<now, created, enabled, running, fires>>

(* Success or failure may finish an execution, without rearming its slot. *)
(* Unknown/nonterminating executions may remain running forever.         *)
Finish(s) ==
    /\ running[s] > 0
    /\ running' = [running EXCEPT ![s] = @ - 1]
    /\ UNCHANGED <<now, created, enabled, nextDue, fires>>

ObservedDue(s, t) ==
    IF created[s] /\ Interval[s] > 0 /\ nextDue[s] < t /\ ~KeepMissed
    THEN nextDue[s] + (((t - nextDue[s] - 1) \div Interval[s]) + 1) * Interval[s]
    ELSE nextDue[s]

Advance(t) ==
    /\ t \in (now + 1)..MaxTime
    /\ \A s \in Schedules : ~Due(s)
    (* Retain TaskSchedule's undecided overdue-one-time scope restriction. *)
    /\ \A s \in Schedules :
           created[s] /\ enabled[s] /\ Interval[s] = 0 => nextDue[s] >= t
    /\ now' = t
    /\ nextDue' = [s \in Schedules |-> ObservedDue(s, t)]
    /\ UNCHANGED <<created, enabled, running, fires>>

Next ==
    \/ \E s \in Schedules : Create(s) \/ Disable(s) \/ Fire(s)
                              \/ SkipDisabled(s) \/ Finish(s)
    \/ \E t \in 1..MaxTime : Advance(t)

(* Fair scheduler service, but NO assumed execution termination.         *)
Fairness ==
    /\ \A s \in Schedules : WF_vars(Fire(s)) /\ WF_vars(SkipDisabled(s))
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
          /\ fires[i].schedule \in Schedules /\ fires[i].due \in 1..MaxTime
          /\ fires[i].at \in 0..MaxTime /\ fires[i].allowed \in BOOLEAN

NoEarlyFire == \A i \in 1..Len(fires) : fires[i].at >= fires[i].due
NoCatchUp == \A i \in 1..Len(fires) : fires[i].at = fires[i].due
NoDisabledFire == \A i \in 1..Len(fires) : fires[i].allowed
(* An execution outcome never redispatches the same logical occurrence. *)
NoAutomaticRetry ==
    \A o \in Occurrences :
        Cardinality({i \in 1..Len(fires) :
                     <<fires[i].schedule, fires[i].due>> = o}) <= 1
OneTimeAtMostOnce ==
    \A s \in Schedules : Interval[s] = 0 =>
        Cardinality({i \in 1..Len(fires) : fires[i].schedule = s}) <= 1
CadencePreserved ==
    \A s \in Schedules : created[s] /\ Interval[s] > 0 =>
        (nextDue[s] - FirstDue[s]) % Interval[s] = 0
DueEventuallyProcessed == \A s \in Schedules : Due(s) ~> ~Due(s)

(* Diagnostic only: expected to FAIL, witnessing intentionally allowed    *)
(* overlap between distinct periodic occurrences. Not a design invariant.*)
NoOverlapDiagnostic == \A s \in Schedules : running[s] <= 1
=============================================================================
