------------------------- MODULE RunnerOwnership -------------------------
EXTENDS Naturals, Sequences, FiniteSets, TLC
CONSTANTS MaxSteps, MaxFaults, ReplayUnknown, IndexOnlyAck, Target
Nodes == 1..2
None == 0
Msg(kind, n, t) == [kind |-> kind, node |-> n, term |-> t]
Event(op, n, arg) == [op |-> op, node |-> n, arg |-> arg]
VARIABLES step, event, now, epoch, leader, quorum, up, owned, receipt,
          pending, pendingTerm, waiting, tickets, committedOwner, committedTerm,
          consumes, posts, net, work, admissions, staleAdmission, badAck, faults
vars == <<step,event,now,epoch,leader,quorum,up,owned,receipt,pending,pendingTerm,
          waiting,tickets,committedOwner,committedTerm,consumes,posts,net,work,
          admissions,staleAdmission,badAck,faults>>
Init == /\ step = 0 /\ event = Event("init",0,0) /\ now = 0
        /\ epoch = 1 /\ leader = 1 /\ quorum = TRUE /\ up = Nodes
        /\ owned = [n \in Nodes |-> 0] /\ receipt = "Pending"
        /\ pending = 0 /\ pendingTerm = 0 /\ waiting = [n \in Nodes |-> 0]
        /\ tickets = [n \in Nodes |-> 0]
        /\ committedOwner = 0 /\ committedTerm = 0
        /\ consumes = 0 /\ posts = 0 /\ net = <<>> /\ work = <<>>
        /\ admissions = 0 /\ staleAdmission = FALSE /\ badAck = FALSE /\ faults = 0
Tag(op,n,arg) == /\ step' = step + 1 /\ event' = Event(op,n,arg)
Authority(n) == n = leader /\ n \in up /\ owned[n] = epoch
Remove(s,i) == SubSeq(s,1,i-1) \o SubSeq(s,i+1,Len(s))
Tick == /\ now = 0 /\ now' = 1 /\ Tag("tick",0,1)
        /\ UNCHANGED <<epoch,leader,quorum,up,owned,receipt,pending,pendingTerm,waiting,tickets,committedOwner,committedTerm,consumes,posts,net,work,admissions,staleAdmission,badAck,faults>>
Acquire(n) == /\ n = leader /\ n \in up /\ quorum /\ owned[n] # epoch
 /\ owned' = [owned EXCEPT ![n] = epoch]
 /\ receipt' = (IF pending > 0 THEN "Unknown" ELSE IF receipt \in {"Running","Unknown"} THEN IF ReplayUnknown THEN "Pending" ELSE "Unknown" ELSE receipt)
 /\ consumes' = consumes + (IF pending > 0 THEN 1 ELSE 0)
 /\ committedOwner' = (IF pending > 0 THEN pending ELSE committedOwner)
 /\ committedTerm' = (IF pending > 0 THEN pendingTerm ELSE committedTerm)
 /\ pending' = 0
 /\ Tag("acquire",n,epoch)
 /\ UNCHANGED <<now,epoch,leader,quorum,up,pendingTerm,waiting,tickets,posts,net,work,admissions,staleAdmission,badAck,faults>>
Propose(n) == /\ Authority(n) /\ now = 1 /\ receipt = "Pending" /\ pending = 0 /\ consumes < 2
 /\ pending' = n /\ pendingTerm' = epoch /\ waiting' = [waiting EXCEPT ![n] = epoch]
 /\ Tag("propose",n,epoch)
 /\ UNCHANGED <<now,epoch,leader,quorum,up,owned,receipt,tickets,committedOwner,committedTerm,consumes,posts,net,work,admissions,staleAdmission,badAck,faults>>
Commit == /\ pending # 0 /\ pending = leader /\ pending \in up /\ pendingTerm = epoch /\ quorum
 /\ receipt' = "Running" /\ consumes' = consumes + 1
 /\ committedOwner' = pending /\ committedTerm' = pendingTerm /\ pending' = 0
 /\ Tag("commit",pending,epoch)
 /\ UNCHANGED <<now,epoch,leader,quorum,up,owned,pendingTerm,waiting,tickets,posts,net,work,admissions,staleAdmission,badAck,faults>>
Ack(n) == /\ n \in up /\ waiting[n] > 0 /\ committedOwner > 0
 /\ LET valid == committedOwner = n /\ committedTerm = waiting[n] /\ epoch = waiting[n] /\ leader = n
        accepted == valid \/ IndexOnlyAck
    IN /\ tickets' = (IF accepted THEN [tickets EXCEPT ![n] = waiting[n]] ELSE tickets)
       /\ badAck' = (badAck \/ (accepted /\ ~valid))
 /\ waiting' = [waiting EXCEPT ![n] = 0]
 /\ Tag("ack",n,waiting[n])
 /\ UNCHANGED <<now,epoch,leader,quorum,up,owned,receipt,pending,pendingTerm,committedOwner,committedTerm,consumes,posts,net,work,admissions,staleAdmission,faults>>
LoseAck(n) == /\ waiting[n] > 0 /\ faults < MaxFaults
 /\ owned' = [owned EXCEPT ![n] = 0]
 /\ waiting' = [waiting EXCEPT ![n] = 0] /\ faults' = faults + 1 /\ Tag("lose_ack",n,0)
 /\ UNCHANGED <<now,epoch,leader,quorum,up,receipt,pending,pendingTerm,tickets,committedOwner,committedTerm,consumes,posts,net,work,admissions,staleAdmission,badAck>>
Send(n) == /\ n \in up /\ tickets[n] > 0 /\ Len(net) < 2 /\ posts < 2
 /\ net' = Append(net, Msg("post",n,tickets[n])) /\ tickets' = [tickets EXCEPT ![n] = 0]
 /\ posts' = posts + 1 /\ Tag("send",n,tickets[n])
 /\ UNCHANGED <<now,epoch,leader,quorum,up,owned,receipt,pending,pendingTerm,waiting,committedOwner,committedTerm,consumes,work,admissions,staleAdmission,badAck,faults>>
DeliverPost(i) == /\ i \in 1..Len(net) /\ net[i].kind = "post" /\ Len(work) < 2 /\ admissions < 2
 /\ work' = Append(work,net[i]) /\ net' = Remove(net,i) /\ admissions' = admissions + 1
 /\ staleAdmission' = (staleAdmission \/ net[i].term # epoch)
 /\ Tag("deliver_post",net[i].node,i)
 /\ UNCHANGED <<now,epoch,leader,quorum,up,owned,receipt,pending,pendingTerm,waiting,tickets,committedOwner,committedTerm,consumes,posts,badAck,faults>>
Finish(i) == /\ i \in 1..Len(work) /\ Len(net) < 2
 /\ net' = Append(net,Msg("done",work[i].node,work[i].term)) /\ work' = Remove(work,i)
 /\ Tag("finish",work[i].node,i)
 /\ UNCHANGED <<now,epoch,leader,quorum,up,owned,receipt,pending,pendingTerm,waiting,tickets,committedOwner,committedTerm,consumes,posts,admissions,staleAdmission,badAck,faults>>
DeliverDone(i,lost) == /\ i \in 1..Len(net) /\ net[i].kind = "done"
 /\ (~Authority(net[i].node) \/ net[i].term # epoch \/ quorum)
 /\ receipt' = (IF Authority(net[i].node) /\ net[i].term = epoch /\ quorum /\ receipt = "Running" THEN "Completed" ELSE receipt)
 /\ owned' = (IF lost /\ Authority(net[i].node) /\ net[i].term = epoch /\ quorum /\ receipt = "Running" THEN [owned EXCEPT ![net[i].node] = 0] ELSE owned)
 /\ net' = Remove(net,i) /\ Tag(IF lost THEN "deliver_done_lost_ack" ELSE "deliver_done",net[i].node,i)
 /\ UNCHANGED <<now,epoch,leader,quorum,up,pending,pendingTerm,waiting,tickets,committedOwner,committedTerm,consumes,posts,work,admissions,staleAdmission,badAck,faults>>
Drop(i) == /\ i \in 1..Len(net) /\ faults < MaxFaults
 /\ net' = Remove(net,i) /\ faults' = faults + 1 /\ Tag("drop",0,i)
 /\ UNCHANGED <<now,epoch,leader,quorum,up,owned,receipt,pending,pendingTerm,waiting,tickets,committedOwner,committedTerm,consumes,posts,work,admissions,staleAdmission,badAck>>
Duplicate(i) == /\ i \in 1..Len(net) /\ Len(net) < 2 /\ faults < MaxFaults
 /\ net' = Append(net,net[i]) /\ faults' = faults + 1 /\ Tag("duplicate",0,i)
 /\ UNCHANGED <<now,epoch,leader,quorum,up,owned,receipt,pending,pendingTerm,waiting,tickets,committedOwner,committedTerm,consumes,posts,work,admissions,staleAdmission,badAck>>
Partition == /\ quorum /\ faults < MaxFaults /\ quorum' = FALSE /\ faults' = faults + 1 /\ Tag("partition",0,0)
 /\ UNCHANGED <<now,epoch,leader,up,owned,receipt,pending,pendingTerm,waiting,tickets,committedOwner,committedTerm,consumes,posts,net,work,admissions,staleAdmission,badAck>>
Heal == /\ ~quorum /\ quorum' = TRUE /\ Tag("heal",0,0)
 /\ UNCHANGED <<now,epoch,leader,up,owned,receipt,pending,pendingTerm,waiting,tickets,committedOwner,committedTerm,consumes,posts,net,work,admissions,staleAdmission,badAck,faults>>
Elect(n) == /\ epoch = 1 /\ n \in up /\ quorum /\ n # leader
 /\ epoch' = 2 /\ leader' = n /\ pending' = (IF pending = n THEN pending ELSE 0) /\ Tag("elect",n,2)
 /\ UNCHANGED <<now,quorum,up,owned,receipt,pendingTerm,waiting,tickets,committedOwner,committedTerm,consumes,posts,net,work,admissions,staleAdmission,badAck,faults>>
Crash(n) == /\ n \in up /\ faults < MaxFaults
 /\ leader' = (IF leader = n THEN 0 ELSE leader)
 /\ up' = up \ {n} /\ owned' = [owned EXCEPT ![n] = 0] /\ tickets' = [tickets EXCEPT ![n] = 0]
 /\ waiting' = [waiting EXCEPT ![n] = 0] /\ faults' = faults + 1 /\ Tag("crash",n,0)
 /\ UNCHANGED <<now,epoch,quorum,receipt,pending,pendingTerm,committedOwner,committedTerm,consumes,posts,net,work,admissions,staleAdmission,badAck>>
Restart(n) == /\ n \notin up /\ up' = up \cup {n} /\ Tag("restart",n,0)
 /\ UNCHANGED <<now,epoch,leader,quorum,owned,receipt,pending,pendingTerm,waiting,tickets,committedOwner,committedTerm,consumes,posts,net,work,admissions,staleAdmission,badAck,faults>>
Next == /\ step < MaxSteps
        /\ (Tick \/ Commit \/ Partition \/ Heal
          \/ \E n \in Nodes : Acquire(n) \/ Propose(n) \/ Ack(n) \/ LoseAck(n) \/ Send(n) \/ Elect(n) \/ Crash(n) \/ Restart(n)
          \/ \E i \in 1..2 : DeliverPost(i) \/ Finish(i) \/ DeliverDone(i,FALSE) \/ DeliverDone(i,TRUE) \/ Drop(i) \/ Duplicate(i))
Spec == Init /\ [][Next]_vars
TypeOK == /\ step \in 0..MaxSteps /\ now \in 0..1 /\ epoch \in 1..2 /\ leader \in 0..2 /\ up \subseteq Nodes
          /\ receipt \in {"Pending","Running","Unknown","Completed"} /\ posts \in 0..2 /\ consumes \in 0..2 /\ admissions \in 0..2
NoAutomaticReplay == posts <= 1
NoUncommittedSubmission == posts > 0 => consumes > 0
NoFalseClaimAck == ~badAck
NoStaleAdmissionDiagnostic == ~staleAdmission
NoDuplicateAdmissionDiagnostic == admissions <= 1
WitnessAbsent == CASE Target = "recovered_lost_ack" -> ~(receipt = "Unknown" /\ epoch = 1 /\ consumes = 1)
 [] Target = "recovered_restart" -> ~(receipt = "Unknown" /\ epoch = 2 /\ leader = committedOwner)
 [] Target = "healed_pending_claim" -> ~(event.op = "heal" /\ pending > 0)
 [] Target = "stale_completion" -> ~(event.op = "deliver_done" /\ epoch = 2)
 [] OTHER -> event.op # Target
=============================================================================
