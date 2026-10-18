------------------------- MODULE RunnerAdmission -------------------------
EXTENDS Naturals, Sequences, FiniteSets, TLC
CONSTANTS MaxSteps, SkipEpoch, SkipDedup, Target
Workers == 1..2
Event(op,w) == [op |-> op, worker |-> w]
VARIABLES step, event, epoch, claimed, active, quorum, net, reserved, attempted, admitted, grants, consumed, ready, up, launches, lateLaunch, badAdmission
vars == <<step,event,epoch,claimed,active,quorum,net,reserved,attempted,admitted,grants,consumed,ready,up,launches,lateLaunch,badAdmission>>
Init == step = 0 /\ event = Event("init",0) /\ epoch = 1 /\ claimed = FALSE /\ active = TRUE /\ quorum = TRUE /\ net = 0 /\ reserved = {} /\ attempted = {} /\ admitted = {} /\ grants = {} /\ consumed = {} /\ ready = {} /\ up = Workers /\ launches = 0 /\ lateLaunch = FALSE /\ badAdmission = FALSE
Claim == /\ ~claimed /\ epoch = 1 /\ active /\ quorum
 /\ claimed' = (TRUE)
 /\ step' = (step + 1)
 /\ event' = (Event("claim",0))
 /\ UNCHANGED <<epoch,active,quorum,net,reserved,attempted,admitted,grants,consumed,ready,up,launches,lateLaunch,badAdmission>>
Send == /\ claimed /\ net = 0 /\ reserved = {}
 /\ net' = (1)
 /\ step' = (step + 1)
 /\ event' = (Event("send",0))
 /\ UNCHANGED <<epoch,claimed,active,quorum,reserved,attempted,admitted,grants,consumed,ready,up,launches,lateLaunch,badAdmission>>
Duplicate == /\ net = 1
 /\ net' = (2)
 /\ step' = (step + 1)
 /\ event' = (Event("duplicate",0))
 /\ UNCHANGED <<epoch,claimed,active,quorum,reserved,attempted,admitted,grants,consumed,ready,up,launches,lateLaunch,badAdmission>>
Drop == /\ net > 0
 /\ net' = (net - 1)
 /\ step' = (step + 1)
 /\ event' = (Event("drop",0))
 /\ UNCHANGED <<epoch,claimed,active,quorum,reserved,attempted,admitted,grants,consumed,ready,up,launches,lateLaunch,badAdmission>>
Reserve(w) == /\ w \in up /\ claimed /\ net > 0 /\ w \notin reserved
 /\ net' = (net - 1)
 /\ reserved' = (reserved \cup {w})
 /\ step' = (step + 1)
 /\ event' = (Event("reserve",w))
 /\ UNCHANGED <<epoch,claimed,active,quorum,attempted,admitted,grants,consumed,ready,up,launches,lateLaunch,badAdmission>>
RejectDuplicate(w) == /\ w \in up /\ net > 0 /\ w \in reserved
 /\ net' = (net - 1)
 /\ step' = (step + 1)
 /\ event' = (Event("reject_duplicate",w))
 /\ UNCHANGED <<epoch,claimed,active,quorum,reserved,attempted,admitted,grants,consumed,ready,up,launches,lateLaunch,badAdmission>>
Admit(w) == /\ w \in up /\ w \in reserved /\ w \notin attempted /\ active /\ quorum
 /\ attempted' = (attempted \cup {w})
 /\ admitted' = (IF (epoch = 1 \/ SkipEpoch) /\ (admitted = {} \/ SkipDedup) THEN admitted \cup {w} ELSE admitted)
 /\ grants' = (IF (epoch = 1 \/ SkipEpoch) /\ (admitted = {} \/ SkipDedup) THEN grants \cup {w} ELSE grants)
 /\ badAdmission' = (badAdmission \/ (((epoch = 1 \/ SkipEpoch) /\ (admitted = {} \/ SkipDedup)) /\ epoch # 1))
 /\ step' = (step + 1)
 /\ event' = (Event("admit",w))
 /\ UNCHANGED <<epoch,claimed,active,quorum,net,reserved,consumed,ready,up,launches,lateLaunch>>
LostAck(w) == /\ w \in up /\ w \in reserved /\ w \notin attempted /\ active /\ quorum /\ epoch = 1 /\ admitted = {}
 /\ attempted' = (attempted \cup {w})
 /\ admitted' = ({w})
 /\ active' = (FALSE)
 /\ step' = (step + 1)
 /\ event' = (Event("lost_ack",w))
 /\ UNCHANGED <<epoch,claimed,quorum,net,reserved,grants,consumed,ready,up,launches,lateLaunch,badAdmission>>
Uncertain(w) == /\ w \in up /\ w \in reserved /\ w \notin attempted /\ active /\ ~quorum
 /\ attempted' = (attempted \cup {w})
 /\ active' = (FALSE)
 /\ step' = (step + 1)
 /\ event' = (Event("uncertain",w))
 /\ UNCHANGED <<epoch,claimed,quorum,net,reserved,admitted,grants,consumed,ready,up,launches,lateLaunch,badAdmission>>
Consume(w) == /\ w \in up /\ w \in grants /\ w \notin consumed
 /\ grants' = (grants \ {w})
 /\ consumed' = (consumed \cup {w})
 /\ ready' = (ready \cup {w})
 /\ step' = (step + 1)
 /\ event' = (Event("consume",w))
 /\ UNCHANGED <<epoch,claimed,active,quorum,net,reserved,attempted,admitted,up,launches,lateLaunch,badAdmission>>
Launch(w) == /\ w \in up /\ w \in ready /\ launches < 2
 /\ ready' = (ready \ {w})
 /\ launches' = (launches + 1)
 /\ lateLaunch' = (lateLaunch \/ epoch = 2)
 /\ step' = (step + 1)
 /\ event' = (Event("launch",w))
 /\ UNCHANGED <<epoch,claimed,active,quorum,net,reserved,attempted,admitted,grants,consumed,up,badAdmission>>
Crash(w) == /\ w \in up
 /\ attempted' = (IF w \in reserved THEN attempted \cup {w} ELSE attempted)
 /\ up' = (up \ {w})
 /\ grants' = (grants \ {w})
 /\ ready' = (ready \ {w})
 /\ step' = (step + 1)
 /\ event' = (Event("crash",w))
 /\ UNCHANGED <<epoch,claimed,active,quorum,net,reserved,admitted,consumed,launches,lateLaunch,badAdmission>>
Restart(w) == /\ w \notin up
 /\ up' = (up \cup {w})
 /\ step' = (step + 1)
 /\ event' = (Event("restart",w))
 /\ UNCHANGED <<epoch,claimed,active,quorum,net,reserved,attempted,admitted,grants,consumed,ready,launches,lateLaunch,badAdmission>>
Partition == /\ quorum
 /\ quorum' = (FALSE)
 /\ step' = (step + 1)
 /\ event' = (Event("partition",0))
 /\ UNCHANGED <<epoch,claimed,active,net,reserved,attempted,admitted,grants,consumed,ready,up,launches,lateLaunch,badAdmission>>
Heal == /\ ~quorum
 /\ quorum' = (TRUE)
 /\ step' = (step + 1)
 /\ event' = (Event("heal",0))
 /\ UNCHANGED <<epoch,claimed,active,net,reserved,attempted,admitted,grants,consumed,ready,up,launches,lateLaunch,badAdmission>>
Takeover == /\ epoch = 1 /\ quorum
 /\ epoch' = (2)
 /\ active' = (TRUE)
 /\ step' = (step + 1)
 /\ event' = (Event("takeover",0))
 /\ UNCHANGED <<claimed,quorum,net,reserved,attempted,admitted,grants,consumed,ready,up,launches,lateLaunch,badAdmission>>
OwnerCrash == /\ active
 /\ active' = (FALSE)
 /\ step' = (step + 1)
 /\ event' = (Event("owner_crash",0))
 /\ UNCHANGED <<epoch,claimed,quorum,net,reserved,attempted,admitted,grants,consumed,ready,up,launches,lateLaunch,badAdmission>>
Next == /\ step < MaxSteps
 /\ (Claim \/ Send \/ Duplicate \/ Drop \/ Partition \/ Heal \/ Takeover \/ OwnerCrash \/ \E w \in Workers : Reserve(w) \/ RejectDuplicate(w) \/ Admit(w) \/ LostAck(w) \/ Uncertain(w) \/ Consume(w) \/ Launch(w) \/ Crash(w) \/ Restart(w))
Spec == Init /\ [][Next]_vars
NoStaleAdmission == ~badAdmission
OneAdmission == Cardinality(admitted) <= 1
OneLaunch == launches <= 1
LaunchRequiresAdmission == launches > 0 => admitted # {}
NoLateLaunchDiagnostic == ~lateLaunch
WitnessAbsent == CASE Target = "consumed_crash" -> ~(event.op = "crash" /\ consumed # {} /\ launches = 0)
 [] Target = "reserved_restart" -> ~(event.op = "restart" /\ event.worker \in reserved)
 [] Target = "late_launch" -> ~lateLaunch
 [] Target = "stale_rejected" -> ~(event.op = "admit" /\ epoch = 2)
 [] Target = "lost_ack_takeover" -> ~(event.op = "takeover" /\ admitted # {} /\ grants = {} /\ consumed = {})
 [] OTHER -> event.op # Target
=============================================================================
