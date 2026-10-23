#!/usr/bin/env python3
"""Render one muxed chapter per checkpoint; measured audio controls beat timing."""
import argparse,json,os,subprocess,shutil,sys
from pathlib import Path
HERE=Path(__file__).resolve().parent
p=argparse.ArgumentParser();p.add_argument('--output',type=Path,required=True);p.add_argument('--sample',action='store_true');a=p.parse_args();out=a.output.resolve();out.mkdir(parents=True,exist_ok=True)
manifest=out/'narration.json'
if not manifest.exists() and not a.sample:raise SystemExit('Generate pinned Phonon narration first; silent render is allowed only with --sample')
data=json.loads((manifest if manifest.exists() else HERE/'storyboard.json').read_text());published=[]
for i,c in enumerate(data['chapters'][:1] if a.sample else data['chapters']):
 env=dict(os.environ,CHAPTER_INDEX=str(i),NARRATION_MANIFEST=str(manifest if manifest.exists() else HERE/'storyboard.json'),VISUAL_SAMPLE='1' if a.sample else '0')
 name=c['id']+('-silent-sample' if a.sample else '')
 subprocess.run([sys.executable,'-m','manim','-ql','--media_dir',str(out/'media'),'-o',name,str(HERE/'scenes.py'),'WalkthroughChapter'],env=env,check=True)
 files=list((out/'media/videos/scenes').glob('*/'+name+'.mp4'))
 if len(files)!=1:raise RuntimeError('Expected one rendered chapter')
 shutil.copy2(files[0],out/(name+'.mp4'));published.append(dict(id=c['id'],title=c['title'],src=name+'.mp4',checkpoint=c['checkpoint'],kind='SILENT VISUAL SAMPLE' if a.sample else c['kind']))
(out/'player-manifest.json').write_text(json.dumps({'chapters':published,'sample':a.sample},indent=2)+'\n')
