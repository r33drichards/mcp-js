---------------------------- MODULE FsLabelAtomicWrite ----------------------------
(***************************************************************************)
(* Abstract model of a cluster-mode label move (server/src/engine/        *)
(* fs_labels.rs).  Every move must update the mutable head pointer AND     *)
(* append a reflog entry recording that move.  fs_reset and the reflog-    *)
(* rooted GC both rely on the reflog covering every head the label has     *)
(* ever pointed at.                                                        *)
(*                                                                         *)
(* We compare two implementations via the CONSTANT `Mode`:                 *)
(*                                                                         *)
(*   "split"    - head and reflog are two separate replicated writes (the  *)
(*                pre-fix code: cas_or_forward / put_or_forward, THEN a     *)
(*                second cl_append_log).  A leader change, timeout, or      *)
(*                crash between the two writes leaves the head moved with   *)
(*                no reflog entry.                                          *)
(*                                                                         *)
(*   "combined" - head and reflog commit inside a single replicated log    *)
(*                entry, applied as one atomic sled batch (the fix:         *)
(*                cas_with / put_with carrying `extra` writes).             *)
(*                                                                         *)
(* Model-check result (TLC):                                               *)
(*   Mode = "combined"  -> StableReflogCoversHead holds (no error).        *)
(*   Mode = "split"     -> StableReflogCoversHead is violated; TLC returns *)
(*                         a counterexample: StartMove then FailReflog.    *)
(***************************************************************************)
EXTENDS Naturals

CONSTANTS Mode,      \* "split" or "combined"
          MaxMoves   \* bound on the number of moves, to keep the model finite

VARIABLES head,      \* committed head value; 0 is the initial state (label absent)
          reflog,    \* set of values recorded as reflog "to" entries
          phase,     \* "idle", or (split only) "reflogPending"
          pending,   \* value whose reflog append is still outstanding (split)
          moves      \* number of moves started, bounding the behaviour

vars == <<head, reflog, phase, pending, moves>>

Vals == 1..MaxMoves

TypeOK ==
    /\ head    \in 0..MaxMoves
    /\ reflog  \subseteq Vals
    /\ phase   \in {"idle", "reflogPending"}
    /\ pending \in 0..MaxMoves
    /\ moves   \in 0..MaxMoves

Init ==
    /\ head    = 0
    /\ reflog  = {}
    /\ phase   = "idle"
    /\ pending = 0
    /\ moves   = 0

(* Begin a move to a fresh value v = moves+1.  Combined mode advances head  *)
(* and reflog together; split mode commits the head first and leaves the    *)
(* reflog append outstanding.                                               *)
StartMove ==
    /\ phase = "idle"
    /\ moves < MaxMoves
    /\ moves' = moves + 1
    /\ LET v == moves + 1 IN
         IF Mode = "combined"
           THEN /\ head'    = v
                /\ reflog'  = reflog \cup {v}
                /\ phase'   = "idle"
                /\ pending' = 0
           ELSE /\ head'    = v          \* head committed/observable first
                /\ reflog'  = reflog
                /\ phase'   = "reflogPending"
                /\ pending' = v

(* Split mode: the second replicated write lands and records the reflog.    *)
CompleteReflog ==
    /\ Mode = "split"
    /\ phase = "reflogPending"
    /\ reflog'  = reflog \cup {pending}
    /\ phase'   = "idle"
    /\ pending' = 0
    /\ UNCHANGED <<head, moves>>

(* Split mode: the second write fails (leader change / timeout / crash).     *)
(* The head has already moved; the reflog entry is permanently lost.         *)
FailReflog ==
    /\ Mode = "split"
    /\ phase = "reflogPending"
    /\ phase'   = "idle"
    /\ pending' = 0
    /\ UNCHANGED <<head, reflog, moves>>

Next ==
    \/ StartMove
    \/ CompleteReflog
    \/ FailReflog

Spec == Init /\ [][Next]_vars

(***************************************************************************)
(* Safety property.  In any settled (idle) state, a non-initial head must  *)
(* be covered by a reflog entry.  This is exactly what fs_reset/GC assume. *)
(***************************************************************************)
StableReflogCoversHead == (phase = "idle") => ((head = 0) \/ (head \in reflog))

(* The stronger transient form: even mid-operation the head is never        *)
(* observable ahead of the reflog.  Combined mode satisfies this too;       *)
(* split mode violates it the instant StartMove commits the head.           *)
AlwaysReflogCoversHead == (head = 0) \/ (head \in reflog)
===============================================================================
