"""Manual notebook controls. Importing/rendering never calls Docker or a model."""
from pathlib import Path
import json, os, re, subprocess, sys

HERE=Path(__file__).resolve().parent
ACTIONS={'seed','status','policy','query','logs','dashboard','publish','branches','merge','conflict','reset','outage','quorum','pair-test','alert','select-root'}

def run_action(action, project, state, alert_text=''):
    if action not in ACTIONS: raise ValueError('Action not allowed')
    state=Path(state).expanduser().resolve()
    owner=json.loads((state/'ownership.json').read_text())
    if not re.fullmatch(r'mcp-interview-[0-9a-f]{8}',project) or owner['id']!=project:
        raise ValueError('Enter the exact owned demo project ID')
    if action in {'outage','quorum'} and owner.get('node_ids',[1,2,3])!=[1,2,3]:
        raise ValueError('Requires the three-voter phase; do not stop the single-voter investigation')
    env=dict(os.environ,MCP_JS_DEMO_STATE=str(state))
    # No arbitrary commands, alert interpolation, shell, automatic setup or cleanup.
    if action=='alert':
        cmd=[sys.executable,'-c','import sys; import demo; print(demo.ALERTS.intake(sys.stdin.read()))']
    elif action=='select-root':
        if not re.fullmatch(r'[A-Za-z0-9-]{1,100}',alert_text): raise ValueError('Enter a bound session ID')
        cmd=[sys.executable,str(HERE/'select-root.py'),alert_text]
    elif action=='pair-test':cmd=['node',str(HERE/'pi/verify-pairing.mjs')]
    else:cmd=[sys.executable,str(HERE/'demo.py'),action]
    result=subprocess.run(cmd,cwd=HERE,env=env,input=alert_text,text=True,capture_output=True,timeout=None if action in {'outage','quorum'} else 180)
    output=(result.stdout+'\n'+result.stderr).replace(owner.get('token','') or '\0','[REDACTED]')
    if result.returncode: raise RuntimeError(output)
    return output

def panel(actions):
    import ipywidgets as w
    from IPython.display import display
    state=w.Text(value=os.environ.get('MCP_JS_DEMO_STATE',str(HERE/'.state')),description='State path:')
    try: project_id=json.loads((Path(state.value)/'ownership.json').read_text())['id']
    except (OSError,ValueError,KeyError): project_id=''
    project=w.Text(value=project_id,description='Project ID:')
    alert=w.Textarea(value=(HERE/'example-alert.json').read_text(),description='Alert JSON:') if 'alert' in actions else (w.Text(description='Session ID:') if 'select-root' in actions else None)
    confirm=w.Checkbox(value=False,description='Execute one selected action on this owned demo')
    out=w.Output();buttons=[]
    def execute(action):
        with out:
            out.clear_output()
            if not confirm.value:
                print('Not executed: tick confirmation, then click one action.');return
            confirm.value=False
            try: print(run_action(action,project.value,state.value,alert.value if alert else ''))
            except Exception as e: print(type(e).__name__+': '+str(e))
    for action in actions:
        button=w.Button(description=action)
        button.on_click(lambda _,a=action:execute(a));buttons.append(button)
    display(w.VBox([state,project,*([alert] if alert else []),confirm,w.HBox(buttons),out]))
