"""Original Manim Community explanation. Dots/edges simulate flow, never live status."""
import os,json
from pathlib import Path
from manim import *
HERE=Path(__file__).resolve().parent
class WalkthroughChapter(Scene):
 def construct(self):
  self.camera.background_color='#101b23'
  manifest=Path(os.environ.get('NARRATION_MANIFEST',str(HERE/'storyboard.json')))
  data=json.loads(manifest.read_text());idx=int(os.environ.get('CHAPTER_INDEX','0'));c=data['chapters'][idx]
  sample=os.environ.get('VISUAL_SAMPLE')=='1'
  title=Text(c['title'],font_size=34,color='#eaf2ef').to_edge(UP,buff=.45)
  label=Text(('SILENT VISUAL PREVIEW  /  NOT LIVE' if sample or os.environ.get('SILENT_PREVIEW')=='1' else 'EXPLANATORY ANIMATION  /  NOT LIVE'),font_size=16,color='#f4c778').next_to(title,DOWN,buff=.2)
  self.add(title,label)
  boxes=VGroup()
  for name in c['nodes']:
   box=RoundedRectangle(width=2.3,height=1.05,corner_radius=.12,color='#35505c',fill_color='#203844',fill_opacity=1)
   text=Text(name,font_size=19,color='#eaf2ef');text.scale_to_fit_width(min(2.05,text.width));boxes.add(VGroup(box,text))
  boxes.arrange(RIGHT,buff=.3).move_to(UP*.4)
  if boxes.width>13:boxes.scale_to_fit_width(13)
  connections=[(i,i+1) for i in range(len(boxes)-1)]
  if c['id']=='03-fork':
   positions=[LEFT*5,LEFT*2.1+UP*.9,LEFT*2.1+DOWN*.6,RIGHT*1.3,RIGHT*4.7]
   for box,pos in zip(boxes,positions):box.move_to(pos+UP*.4)
   connections=[(0,1),(0,2),(1,3),(2,3),(3,4)]
  edges=VGroup(*[Arrow(boxes[a].get_right(),boxes[b].get_left(),buff=.04,color='#9df0c1',stroke_width=2,max_tip_length_to_length_ratio=.25) for a,b in connections])
  self.play(LaggedStart(*[FadeIn(b) for b in boxes],lag_ratio=.12),Create(edges),run_time=1.2)
  caption=None
  for beat in c['beats'][:1] if sample else c['beats']:
   duration=3.5 if sample else beat.get('duration',5)
   if beat.get('audio') and not sample and os.environ.get('SILENT_PREVIEW')!='1':self.add_sound(str(manifest.parent/beat['audio']))
   next_caption=Text(beat['caption'],font_size=25,color='#eaf2ef');next_caption.scale_to_fit_width(min(12,next_caption.width));next_caption.move_to(DOWN*1.25)
   transitions=[Indicate(boxes[beat['focus']],color='#9df0c1',scale_factor=1.08),FadeIn(next_caption)]
   if caption is not None:transitions.append(FadeOut(caption))
   self.play(*transitions,run_time=min(.8,duration));caption=next_caption
   self.wait(max(.1,duration-.8)+.25)
  stop=Text('PAUSE FOR LIVE DEMO  ·  PRESENTER RESUMES',font_size=21,color='#f4c778').to_edge(DOWN,buff=.6)
  self.play(FadeIn(stop),run_time=.4);self.wait(.5)
