#!/usr/bin/env python3
"""Local stock-voice synthesis. No cloning, hosted inference, or remote Python code."""
import argparse,json,time,wave,urllib.request,hashlib,os
from pathlib import Path
HERE=Path(__file__).resolve().parent
p=argparse.ArgumentParser();p.add_argument('--output',type=Path,required=True);p.add_argument('--resolve-model',action='store_true');p.add_argument('--sample',action='store_true');a=p.parse_args();a.output.mkdir(parents=True,exist_ok=True)
lock=a.output/'model-lock.json'
if a.resolve_model:
 meta=json.load(urllib.request.urlopen('https://huggingface.co/api/models/gradium/phonon-31m-en',timeout=30))
 revision=meta['sha'];assert len(revision)==40
 license=meta.get('cardData',{}).get('license')
 if license!='apache-2.0':raise RuntimeError('Unexpected model license; inspect before continuing')
 lock.write_text(json.dumps({'repo':'gradium/phonon-31m-en','revision':revision,'license':license,'files':[x['rfilename'] for x in meta['siblings']]},indent=2))
if not lock.exists():raise SystemExit('No immutable model pin. Run --resolve-model on a host with authorized Hugging Face access; cloud access was HTTP 403.')
meta=json.loads(lock.read_text());os.environ.setdefault('HF_HOME',str(a.output/'hf-cache'))
import ptts
start=time.monotonic();tts=ptts.TTS('hf://'+meta['repo']+'@'+meta['revision'],lang='en',quant='q8')
if 'Easton' not in tts.voices:raise RuntimeError('Requested stock voice Easton absent; do not substitute silently')
board=json.loads((HERE/'storyboard.json').read_text());result={'mode':'NARRATED EXPLANATORY ANIMATION','model':meta,'ptts':'0.4.1','voice':'Easton','chapters':[]}
for c in board['chapters'][:1] if a.sample else board['chapters']:
 out=dict(c);out['beats']=[]
 for i,beat in enumerate(c['beats'][:1] if a.sample else c['beats']):
  path=a.output/f'{c["id"]}-{i}.wav';t=time.monotonic();tts.save(str(path),beat['speech'],voice='Easton')
  with wave.open(str(path)) as f:seconds=f.getnframes()/f.getframerate();assert f.getframerate()==24000 and f.getnchannels()==1
  out['beats'].append(dict(beat,audio=path.name,duration=seconds,sha256=hashlib.sha256(path.read_bytes()).hexdigest(),synthesis_seconds=time.monotonic()-t))
 result['chapters'].append(out)
result['elapsed_seconds']=time.monotonic()-start;(a.output/'narration.json').write_text(json.dumps(result,indent=2)+'\n');print(json.dumps({'elapsed_seconds':result['elapsed_seconds'],'chapters':len(result['chapters'])}))
