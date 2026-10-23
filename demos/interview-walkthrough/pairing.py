"""Custom Pi-session -> mcp-js branch adapter, not a built-in Pi feature.
All mutations are serialized by the controller lock. Pending/failed forks stay
in the journal. An unbound session cannot execute. Only this adapter chooses fs.
"""
import json, time, uuid
class Pairing:
    def __init__(self, demo): self.d=demo
    def load(self):
        p=self.d.STATE/'pairs.json'
        return json.loads(p.read_text()) if p.exists() else {'sessions':{},'forks':{}}
    def save(self,s): self.d.save(self.d.STATE/'pairs.json',s)
    def bind(self,s,sid,root,label):
        code,r=self.d.request(self.d.leader(),'/api/fs/push',{'label':label,'ca_id':root})
        if code!=200:
            # Resolve before any retry; network/commit ambiguity must not create another label.
            _,r=self.d.request(self.d.leader(),'/api/fs/labels/'+label)
            if r.get('ca_id')!=root: raise RuntimeError('Binding publish uncertain; inspect journal '+label)
        s['sessions'][sid]={'root':root,'base':root,'label':label,'updated':time.time()}
    def action(self,action,args):
        s=self.load()
        if action=='inspect': return s
        if action=='init':
            sid=args['session']
            if sid not in s['sessions']:
                base=self.d.state_data().get('B')
                if not base: raise RuntimeError('Run seed before /demo-init')
                label='pi-'+uuid.uuid4().hex
                s['forks'][label]={'status':'initializing','target':sid,'root':base,'label':label}; self.save(s)
                self.bind(s,sid,base,label); s['forks'][label]['status']='committed'; self.save(s)
            return s['sessions'][sid]
        if action=='prepare':
            parent=args['session']
            if parent not in s['sessions']: raise RuntimeError('Unbound parent')
            key=uuid.uuid4().hex
            s['forks'][key]={'status':'prepared','parent':parent,'root':s['sessions'][parent]['root'],'label':'pi-'+key,'created':time.time()}
            self.save(s); return {'reservation':key,'base':s['forks'][key]['root']}
        if action=='abort':
            f=s['forks'][args['reservation']]
            if f['status']=='prepared': f['status']='aborted'; self.save(s)
            return f
        if action=='commit':
            f=s['forks'][args['reservation']]; sid=args['session']
            if f['status']=='committed':
                if f['target']!=sid: raise RuntimeError('Reservation belongs to another session')
                return s['sessions'][sid]
            if f['status'] not in ('prepared','committing'): raise RuntimeError('Fork not committable')
            if sid==f['parent'] or sid in s['sessions']: raise RuntimeError('Target must be a fresh Pi session')
            f.update(status='committing',target=sid); self.save(s)
            self.bind(s,sid,f['root'],f['label'])
            s['sessions'][sid]['parent']=f['parent']; f['status']='committed'; self.save(s)
            return s['sessions'][sid]
        raise RuntimeError('Unknown pairing action')
    def call(self,sid,name,args):
        s=self.load()
        if sid not in s['sessions']: raise RuntimeError('UNBOUND session: use /demo-init or /paired-fork; no filesystem access granted')
        b=s['sessions'][sid]
        if name=='branch_info': return dict(b,session=sid)
        if name!='run_js': raise RuntimeError('Tool not exposed by branch adapter')
        if set(args)-{'code'}: raise RuntimeError('fs/session/heap override forbidden by branch adapter')
        code=args.get('code')
        if not isinstance(code,str) or len(code)>32000: raise RuntimeError('Expected code string <=32K')
        result=self.d.exec_js(code,b['root'],4)
        if result['status']!='completed': return result
        status,p=self.d.request(self.d.leader(),'/api/fs/push',{'label':b['label'],'ca_id':result['fs'],'expected':b['root']})
        if status!=200:
            self.d.record('pair_publish_uncertain',{'session':sid,'execution':result,'response':p})
            raise RuntimeError('Branch publish failed/uncertain; output root recorded, no automatic overwrite')
        b['root']=result['fs']; b['updated']=time.time(); self.save(s)
        return result
    def merge(self,args):
        s=self.load(); target=s['sessions'][args['target']]; other=s['sessions'][args['other']]
        # A direct child records the parent's root at fork time, even if the
        # parent has written since init. Never substitute the parent's init base.
        if other.get('parent')==args['target']:
            base=other['base']
        elif target.get('parent')==args['other']:
            base=target['base']
        elif target['base']==other['base']:
            base=target['base']
        else:
            raise RuntimeError('Different bases; this adapter requires an explicit external reconciliation')
        code,r=self.d.request(self.d.leader(),'/api/fs/merge',{'ours':target['root'],'theirs':other['root'],'base':base})
        if r.get('status')=='conflict': return r
        if code!=200 or 'ca_id' not in r: raise RuntimeError('Merge failed '+str(r))
        code,p=self.d.request(self.d.leader(),'/api/fs/push',{'label':target['label'],'ca_id':r['ca_id'],'expected':target['root']})
        if code!=200: raise RuntimeError('CAS rejected; preserve both branch roots')
        target['root']=r['ca_id']; self.save(s)
        return {'fs':r['ca_id'],'conversation_histories':'unchanged','target':args['target']}
