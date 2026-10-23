import unittest
from pairing import Pairing

class Demo:
    def __init__(self): self.requests=[]
    def leader(self): return 1
    def request(self,node,path,args):
        self.requests.append((path,args))
        return 200, {'ca_id':'merged'}

class MergeBaseTests(unittest.TestCase):
    def check(self,target,other,expected):
        d=Demo();p=Pairing(d)
        state={'sessions':{'P':{'base':'B','root':'P-after','label':'p'},'C':{'base':'R-at-fork','root':'C-after','label':'c','parent':'P'}}}
        p.load=lambda:state;p.save=lambda s:None
        p.merge({'target':target,'other':other})
        self.assertEqual(d.requests[0][1]['base'],expected)
        self.assertEqual(d.requests[1][1]['expected'],target+'-after')
    def test_child_into_parent_after_parent_writes(self): self.check('P','C','R-at-fork')
    def test_parent_into_child_after_parent_writes(self): self.check('C','P','R-at-fork')
    def test_unrelated_different_bases_rejected(self):
        p=Pairing(Demo());p.load=lambda:{'sessions':{'A':{'base':'a'},'B':{'base':'b'}}}
        with self.assertRaisesRegex(RuntimeError,'Different bases'):p.merge({'target':'A','other':'B'})

if __name__=='__main__':unittest.main()
