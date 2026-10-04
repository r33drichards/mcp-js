#!/usr/bin/env python3
import argparse,collections,json,pathlib,sys
OK={"pass","platform_inapplicable"}
def main():
 p=argparse.ArgumentParser();p.add_argument("--inventory",type=pathlib.Path,required=True);p.add_argument("--versions",type=pathlib.Path,required=True);p.add_argument("--results-dir",type=pathlib.Path,required=True);p.add_argument("--shard-total",type=int,required=True);p.add_argument("--json-output",type=pathlib.Path,required=True);p.add_argument("--markdown-output",type=pathlib.Path,required=True);a=p.parse_args()
 try:
  inv=json.loads(a.inventory.read_text());ver=json.loads(a.versions.read_text())["deno_node_test"];summ=[json.loads(x.read_text()) for x in a.results_dir.rglob("summary.json")]
  if sorted(x.get("shard_index") for x in summ)!=list(range(a.shard_total)):raise ValueError("missing or duplicate shard summaries")
  rows=[]
  for f in sorted(a.results_dir.rglob("results.jsonl")):
   rows.extend(json.loads(x) for x in f.read_text().splitlines() if x.strip())
  expected={x["path"] for x in inv["tests"]};seen=[x["path"] for x in rows]
  if len(seen)!=len(set(seen)):raise ValueError("duplicate result paths")
  missing=expected-set(seen)
  if missing:raise ValueError(f"missing {len(missing)} results")
  if set(seen)-expected:raise ValueError("unexpected result paths")
  if any(x["corpus_commit"]!=ver["commit"] or x["node_version"]!=ver["node_version"] for x in rows):raise ValueError("version mismatch")
  status=collections.Counter(x["status"] for x in rows);fail=sorted((x for x in rows if x["status"] not in OK),key=lambda x:x["path"]);report={"schema_version":1,"total":len(rows),"applicable":len(rows)-status.get("platform_inapplicable",0),"passing":status.get("pass",0),"failing":len(fail),"platform_inapplicable":status.get("platform_inapplicable",0),"status":dict(sorted(status.items())),"failures":fail}
  a.json_output.parent.mkdir(parents=True,exist_ok=True);a.json_output.write_text(json.dumps(report,indent=2)+"\n");lines=["# Full Linux Node Compatibility","",f"**{report['passing']} passing / {report['applicable']} applicable; {report['failing']} failing.**","","| Status | Count |","|---|---:|",*[f"| `{k}` | {v} |" for k,v in report["status"].items()],"","## First 200 Failures","",*[f"- `{x['path']}` — `{x['status']}`" for x in fail[:200]]];a.markdown_output.write_text("\n".join(lines)+"\n");return 1 if fail else 0
 except Exception as e:print(f"node compat aggregation error: {e}",file=sys.stderr);return 2
if __name__=="__main__":raise SystemExit(main())
