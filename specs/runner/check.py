#!/usr/bin/env python3
"""Run bounded TLC checks and export normalized generated traces for Rust replay."""
import argparse, hashlib, json, pathlib, re, subprocess, tempfile
ROOT = pathlib.Path(__file__).resolve().parent
SHA = 'd15655eeaff4b3f7b3d65c46b838bc0f35d131b2ef4b1e473689b62c98d4726f'
p = argparse.ArgumentParser(); p.add_argument('--jar', required=True); p.add_argument('--out', default=str(ROOT/'generated')); p.add_argument('--deep', action='store_true'); args=p.parse_args()
jar=pathlib.Path(args.jar).resolve(); assert hashlib.sha256(jar.read_bytes()).hexdigest()==SHA, 'Unexpected TLC binary: pin/check the tool before changing this checksum'
out=pathlib.Path(args.out).resolve(); out.mkdir(parents=True,exist_ok=True)
steps,faults=(16,3) if args.deep else (12,2)
ops=['tick','acquire','propose','commit','ack','lose_ack','send','deliver_post','finish','deliver_done','deliver_done_lost_ack','drop','duplicate','partition','heal','elect','crash','restart']
summary=[]
def run(name, *, target='none', invariant=None, replay=False, index=False):
    with tempfile.TemporaryDirectory() as scratch:
        scratch=pathlib.Path(scratch)
        cfg=scratch/'run.cfg'
        invariants='TypeOK NoAutomaticReplay NoUncommittedSubmission NoFalseClaimAck'
        if invariant: invariants+=' '+invariant
        cfg.write_text(f'SPECIFICATION Spec\nCONSTANTS MaxSteps = {steps} MaxFaults = {faults} ReplayUnknown = {str(replay).upper()} IndexOnlyAck = {str(index).upper()} Target = "{target}"\nINVARIANTS {invariants}\nCHECK_DEADLOCK FALSE\n')
        raw=out/f'{name}.tlc.json'
        cmd=['java','-XX:+UseParallelGC','-Xmx2g','-cp',str(jar),'tlc2.TLC','-workers','1','-seed','1','-fp','0','-noGenerateSpecTE','-metadir',str(scratch/'states'),'-dumpTrace','json',str(raw),'-config',str(cfg),str(ROOT/'RunnerOwnership.tla')]
        result=subprocess.run(cmd,capture_output=True,text=True,timeout=180 if args.deep else 60)
        log=result.stdout+result.stderr; (out/f'{name}.log').write_text(log)
        expected='NoAutomaticReplay' if replay else 'NoFalseClaimAck' if index else invariant
        if expected:
            assert result.returncode==12 and f'Invariant {expected} is violated.' in log, (name,result.returncode,log[-2000:])
            data=json.loads(raw.read_text()); edges=data['counterexample']['action']; states=[edges[0][0][1]]+[e[2][1] for e in edges]
            normalized={'format':1,'model':'RunnerOwnership','name':name,'seed':1,'max_steps':steps,'max_faults':faults,'expected_violation':expected,'mutation': 'replay_unknown' if replay else 'index_ack' if index else None,'states':states}
            (out/f'{name}.json').write_text(json.dumps(normalized,indent=2)+'\n')
        else: assert result.returncode==0 and 'No error has been found.' in log, (name,result.returncode,log[-2000:])
        match=re.search(r'([\d,]+) states generated, ([\d,]+) distinct states found',log)
        summary.append({'case':name,'expected':expected or 'safe','generated':int(match[1].replace(',','')),'distinct':int(match[2].replace(',',''))})
        print(name,summary[-1],flush=True)
run('safe')
for op in ops + ['recovered_lost_ack','recovered_restart','healed_pending_claim','stale_completion']: run('witness_'+op,target=op,invariant='WitnessAbsent')
run('diagnostic_stale_start',invariant='NoStaleAdmissionDiagnostic')
run('diagnostic_duplicate_delivery',invariant='NoDuplicateAdmissionDiagnostic')
run('mutant_replay',replay=True)
run('mutant_index_ack',index=True)
(out/'summary.json').write_text(json.dumps({'tlc_sha256':SHA,'model_sha256':hashlib.sha256((ROOT/'RunnerOwnership.tla').read_bytes()).hexdigest(),'seed':1,'max_steps':steps,'max_faults':faults,'cases':summary},indent=2)+'\n')
