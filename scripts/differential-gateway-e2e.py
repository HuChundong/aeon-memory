#!/usr/bin/env python3
"""Black-box differential runner for the pinned TS and Rust gateways.

No implementation is imported.  Every observation crosses the process/HTTP
boundary, and durable state is inspected only after processes have stopped.
"""
from __future__ import annotations

import argparse, contextlib, http.client, json, os, pathlib, shutil, signal
import socket, sqlite3, subprocess, tempfile, time, urllib.error, urllib.request, hashlib
from collections import Counter

ORACLE_COMMIT = "4339e63650920871eb0e8888083a1779d114e3ae"

class Failure(Exception): pass

def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0)); return s.getsockname()[1]

def request(base, method, route, value=None, headers=None, raw=None, timeout=20):
    data = raw if raw is not None else (None if value is None else json.dumps(value, ensure_ascii=False).encode())
    h = dict(headers or {})
    if data is not None: h.setdefault("content-type", "application/json")
    req = urllib.request.Request(base + route, data=data, method=method, headers=h)
    started = time.monotonic()
    try:
        with urllib.request.urlopen(req, timeout=timeout) as response:
            body = response.read(); status = response.status; rh = dict(response.headers.items())
    except urllib.error.HTTPError as error:
        body = error.read(); status = error.code; rh = dict(error.headers.items())
    elapsed = time.monotonic() - started
    try: parsed = json.loads(body)
    except Exception: parsed = body.decode("utf-8", "replace")
    return {"status": status, "body": parsed, "headers": {k.lower():v for k,v in rh.items()}, "elapsed": elapsed}

def normalize(response):
    import re
    def visit(value):
        if isinstance(value, dict): return {k:visit(v) for k,v in value.items()}
        if isinstance(value, list): return [visit(v) for v in value]
        if isinstance(value, str):
            return re.sub(r"\[\d{4}-\d\d-\d\dT[^\]]+\]", "[TIMESTAMP]", value)
        return value
    value = visit(json.loads(json.dumps(response["body"])))
    if isinstance(value, dict):
        value.pop("uptime", None); value.pop("duration_ms", None)
        if "output_dir" in value: value["output_dir"] = "<SEED_DIR>"
    return {"status": response["status"], "body": value}

def yaml_config(port, data, mock_port, overrides="", auth=True):
    key = "parity-secret" if auth else ""
    api = f'  apiKey: "{key}"\n' if auth else ""
    pipeline = overrides or '''  pipeline:\n    everyNConversations: 5\n    enableWarmup: true\n    l1IdleTimeoutSeconds: 30\n    l2DelayAfterL1Seconds: 1\n    l2MinIntervalSeconds: 0\n    l2MaxIntervalSeconds: 60\n'''
    return f'''server:
  host: 127.0.0.1
  port: {port}
{api}  corsOrigins: ["https://allowed.example"]
data:
  baseDir: "{data}"
llm:
  baseUrl: "http://127.0.0.1:{mock_port}/v1"
  apiKey: "{{KEY}}"
  model: mock
  timeoutMs: 10000
memory:
  embedding:
    provider: none
{pipeline}  persona:
    triggerEveryN: 1
'''

class Pair:
    def __init__(self, repo, baseline, root, mock_port, overrides="", auth=True):
        self.repo, self.baseline, self.root = pathlib.Path(repo), pathlib.Path(baseline), pathlib.Path(root)
        self.mock_port, self.auth = mock_port, auth
        self.ports = {"ts": free_port(), "rs": free_port()}; self.ps = {}
        self.data = {k:self.root/f"{k}-data" for k in ("ts","rs")}
        self.config = {}
        for kind in ("ts","rs"):
            path = self.root/f"{kind}.yaml"
            path.write_text(yaml_config(self.ports[kind], self.data[kind], mock_port, overrides, auth).replace("{KEY}", f"{kind}-key"))
            self.config[kind] = path
    def start(self):
        for kind in ("ts","rs"):
            env = os.environ.copy(); env["AEON_MEMORY_GATEWAY_CONFIG"] = str(self.config[kind])
            if kind == "ts": cmd=["node","--import","tsx","src/gateway/server.ts"]; cwd=self.baseline
            else: cmd=[str(self.repo/"target/debug/aeon-memory-server"),"--config",str(self.config[kind])]; cwd=self.repo
            log=open(self.root/f"{kind}.log","ab", buffering=0)
            self.ps[kind]=subprocess.Popen(cmd,cwd=cwd,env=env,stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
        deadline=time.time()+15
        while time.time()<deadline:
            try:
                if all(request(self.base(k),"GET","/health",timeout=1)["status"]==200 for k in ("ts","rs")): return
            except (OSError, urllib.error.URLError):
                pass
            time.sleep(.1)
        self.stop()
        logs = "\n".join((self.root/f"{k}.log").read_text(errors="replace") for k in ("ts","rs"))
        raise Failure("gateway startup timeout\n"+logs[-8000:])
    def stop(self):
        for p in self.ps.values():
            if p.poll() is None: p.send_signal(signal.SIGINT)
        for p in self.ps.values():
            try:p.wait(5)
            except subprocess.TimeoutExpired: p.kill(); p.wait()
        self.ps={}
    def base(self,k): return f"http://127.0.0.1:{self.ports[k]}"
    def both(self, method, route, value=None, headers=None, raw=None, timeout=20):
        out={}
        for k in ("ts","rs"):
            h=dict(headers or {})
            if self.auth and route != "/health": h.setdefault("authorization","Bearer parity-secret")
            out[k]=request(self.base(k),method,route,value,h,raw,timeout)
        return out

def durable_snapshot(root):
    root=pathlib.Path(root); out={"files":{},"sqlite":{},"cursor_boundaries":{}}
    generated_ids={}
    semantic_ids={}
    id_fields={"id","record_id"}
    source_id_fields={"source_message_ids","sourceMessageIds"}
    wall_clock_fields={
        "createdAt","updatedAt","recordedAt","created_at","updated_at","recorded_at",
        "created_time","updated_time","timestamp_str","timestamp_start","timestamp_end",
        "last_active_time","last_extraction_time","last_extraction_updated_time",
        "l2_last_extraction_time","last_captured_timestamp","last_persona_time",
    }
    clock_groups={
        "recordedAt":"json_recording","recorded_at":"sqlite_recording","last_l1_cursor":"cursor",
        "last_captured_timestamp":"capture",
        "last_active_time":"active",
        "last_extraction_time":"extraction","last_extraction_updated_time":"extraction",
        "l2_last_extraction_time":"extraction",
        "last_persona_time":"persona",
    }
    database=root/"vectors.db"
    if database.is_file():
        try:
            con=sqlite3.connect(database)
            tables={row[0] for row in con.execute("select name from sqlite_master where type='table'")}
            if "l0_conversations" in tables:
                for record_id,session,role,timestamp,content in con.execute(
                    "select record_id,session_key,role,timestamp,message_text from l0_conversations"):
                    digest=hashlib.sha256(str(content).encode()).hexdigest()[:12]
                    semantic_ids[str(record_id)]=f"<L0_ID:{session}:{role}:{timestamp}:{digest}>"
            if "l1_records" in tables:
                for record_id,session,memory_type,content in con.execute(
                    "select record_id,session_key,type,content from l1_records"):
                    digest=hashlib.sha256(str(content).encode()).hexdigest()[:12]
                    semantic_ids[str(record_id)]=f"<L1_ID:{session}:{memory_type}:{digest}>"
            con.close()
        except sqlite3.Error:
            pass
    def generated_id(value):
        if value in (None,""): return value
        token=str(value)
        if token in semantic_ids: return semantic_ids[token]
        if token not in generated_ids: generated_ids[token]=f"<GENERATED_ID_{len(generated_ids)+1}>"
        return generated_ids[token]
    def object_id(value):
        session=value.get("sessionKey",value.get("session_key",""))
        if "role" in value and "timestamp" in value:
            digest=hashlib.sha256(str(value.get("content",value.get("message_text",""))).encode()).hexdigest()[:12]
            return f"<MESSAGE_ID:{session}:{value['role']}:{value['timestamp']}:{digest}>"
        if "content" in value and "type" in value:
            digest=hashlib.sha256(str(value["content"]).encode()).hexdigest()[:12]
            return f"<L1_ID:{session}:{value['type']}:{digest}>"
        return generated_id(value.get("id"))
    def wall_clock(value,group="record"):
        if value in (None,""): return value
        # Cross-process millisecond equality is accidental: two distinct
        # scheduler events may share a tick in one runtime but not the other.
        # Normalize by the named clock domain, while cursor_boundaries below
        # retains the cursor's exact business relationship to persisted L0.
        return f"<WALL_CLOCK_{group.upper()}>"
    def canonical(value,key=None):
        if key in id_fields: return generated_id(value)
        if key in wall_clock_fields or key=="last_l1_cursor": return wall_clock(value,clock_groups.get(key,"record"))
        if key in source_id_fields and isinstance(value,list): return [generated_id(v) for v in value]
        if key=="timestamps" and isinstance(value,list):
            return [wall_clock(v,"record") for v in value]
        if isinstance(value,dict):
            return {k:(object_id(value) if k=="id" else canonical(v,k)) for k,v in sorted(value.items())}
        if isinstance(value,list): return [canonical(v) for v in value]
        return value
    for path in sorted(root.rglob("*")):
        if not path.is_file(): continue
        rel=str(path.relative_to(root))
        if path.name == "vectors.db":
            try: con=sqlite3.connect(path)
            except sqlite3.OperationalError as error:
                out["sqlite"]["<open-error>"]=str(error); continue
            tables={row[0] for row in con.execute("select name from sqlite_master where type='table'")}
            for table in sorted(tables & {"l0_conversations","l1_records","embedding_meta"}):
                cols=[x[1] for x in con.execute(f'pragma table_info("{table}")')]
                # Exact business rows, with generated IDs/times normalized by column name.
                rows=[]
                for row in con.execute(f'select * from "{table}"'):
                    normalized={c:canonical(v,c) for c,v in zip(cols,row)}
                    if isinstance(normalized.get("metadata_json"),str):
                        try: normalized["metadata_json"]=canonical(json.loads(normalized["metadata_json"]))
                        except json.JSONDecodeError: pass
                    rows.append(normalized)
                out["sqlite"][table]=sorted(rows,key=lambda x:json.dumps(x,sort_keys=True,default=str))
            con.close()
        elif path.suffix in {".json",".jsonl"} and ".backup" not in rel:
            text=path.read_text(errors="replace")
            try:
                if path.suffix==".jsonl":
                    values=[canonical(json.loads(line)) for line in text.splitlines() if line.strip()]
                    lines=[json.dumps(v,ensure_ascii=False,sort_keys=True,separators=(",",":")) for v in values]
                    out["files"][rel]="\n".join(sorted(lines))
                else:
                    out["files"][rel]=json.dumps(canonical(json.loads(text)),ensure_ascii=False,sort_keys=True,separators=(",",":"))
            except json.JSONDecodeError:
                out["files"][rel]=text
        elif path.suffix==".md" and ".backup" not in rel:
            import re
            text=path.read_text(errors="replace")
            text=re.sub(r"(?m)^(created|updated):\s*(.+)$",lambda m:f"{m.group(1)}: {wall_clock(m.group(2),'scene')}",text)
            out["files"][rel]=text
    # Cursor values are wall-clock-derived, but their business meaning must not
    # be erased. Record the exact processed boundary relative to persisted L0.
    checkpoint=root/".metadata/recall_checkpoint.json"
    if checkpoint.is_file() and database.is_file():
        try:
            from datetime import datetime
            raw_checkpoint=json.loads(checkpoint.read_text())
            con=sqlite3.connect(database)
            rows=con.execute("select session_key,recorded_at,timestamp from l0_conversations").fetchall(); con.close()
            by_session={}
            for session,recorded,timestamp in rows:
                if isinstance(recorded,str):
                    recorded_ms=int(datetime.fromisoformat(recorded.replace("Z","+00:00")).timestamp()*1000)
                else: recorded_ms=int(recorded)
                by_session.setdefault(session,[]).append((recorded_ms,timestamp))
            for session,state in sorted(raw_checkpoint.get("runner_states",{}).items()):
                cursor=state.get("last_l1_cursor",0); persisted=sorted(by_session.get(session,[]))
                processed=[row for row in persisted if row[0]<=cursor]
                out["cursor_boundaries"][session]={
                    "processed_rows":len(processed),"remaining_rows":len(persisted)-len(processed),
                    "cursor_on_recorded_boundary":any(row[0]==cursor for row in persisted),
                    "max_processed_input_timestamp":max((row[1] for row in processed),default=None),
                }
        except (OSError,ValueError,json.JSONDecodeError,sqlite3.Error) as error:
            out["cursor_boundaries"]={"<error>":str(error)}
    return out

def capture_payload(session, n, text=None):
    u=text or f"turn {n}: remember parity value {n}"
    # A future fixed timestamp is deliberate: the TS recorder's process-start
    # cursor rejects historical messages, while Rust must implement the same
    # cursor semantics.  2030 is stable for the lifetime of this pinned oracle.
    return {"user_content":u,"assistant_content":"acknowledged","session_key":session,"session_id":session,
      "messages":[{"role":"user","content":u,"timestamp":1893456000000+n*2},{"role":"assistant","content":"acknowledged","timestamp":1893456000001+n*2}]}

class Suite:
    def __init__(self,args,work,mock_port): self.args=args; self.work=pathlib.Path(work); self.mock_port=mock_port; self.failures=[]
    def check(self,label,condition,detail=""):
        print(("PASS" if condition else "FAIL"),label)
        if not condition:self.failures.append(f"{label}: {detail}")
    def equal(self,label,pair):
        a,b=normalize(pair["ts"]),normalize(pair["rs"]); self.check(label,a==b,f"TS={a!r} RS={b!r}")
    def mock(self,method,route,value=None): return request(f"http://127.0.0.1:{self.mock_port}",method,route,value)["body"]
    def save_trace(self, name, value):
        (self.work/f"{name}.json").write_text(json.dumps(value,ensure_ascii=False,indent=2))
    def poll_mock(self, label, predicate, timeout=12.0):
        deadline=time.monotonic()+timeout
        last=[]
        while time.monotonic()<deadline:
            last=self.mock("GET","/__log")
            if predicate(last): return last
            time.sleep(.05)
        raise Failure(f"{label}: causal condition not reached within {timeout}s; log={last!r}")
    def sample_mock_until(self, label, predicate, timeout=12.0):
        """Return the last complete trace even on timeout so strict diagnostics still run."""
        deadline=time.monotonic()+timeout
        last=[]
        while time.monotonic()<deadline:
            last=self.mock("GET","/__log")
            if predicate(last):
                self.check(label,True)
                return last
            time.sleep(.05)
        self.check(label,False,f"post-condition not reached within {timeout}s; log={last!r}")
        return last
    @staticmethod
    def provider_events(log, key, tasks=None):
        selected=[x for x in log if x["authorization"]==f"Bearer {key}-key"]
        return selected if tasks is None else [x for x in selected if x["task"] in tasks]
    @contextlib.contextmanager
    def pair(self,name,overrides="",auth=True):
        root=self.work/name; root.mkdir(); p=Pair(self.args.repo,self.args.baseline,root,self.mock_port,overrides,auth)
        try:
            p.start(); yield p
        finally:
            p.stop()
    def contract(self):
      with self.pair("contract") as p:
        self.equal("health",p.both("GET","/health"))
        for route in ["/recall","/capture","/search/memories","/search/conversations","/session/end","/seed"]:
          self.equal(route+" empty object",p.both("POST",route,{}))
          self.equal(route+" invalid JSON",p.both("POST",route,raw=b"{"))
        # The remaining three routes are an approved Rust transport addition.
        # Assert that the pinned oracle stays 404 while Rust exposes a parsed
        # application route (normally 400 for the deliberately empty body).
        for route in ["/offload/before-prompt","/offload/after-tool","/offload/llm-output"]:
          got=p.both("POST",route,{})
          self.check(route+" approved route-surface difference",
                     got["ts"]["status"]==404 and got["rs"]["status"]!=404,
                     f"TS={got['ts']['status']} RS={got['rs']['status']}")
        self.equal("unknown route",p.both("GET","/not-a-route"))
        for token in (None,"Bearer wrong"):
          h={} if token is None else {"authorization":token}
          self.equal("auth "+str(token),p.both("POST","/recall",{"query":"x","session_key":"s"},h))
        for origin in ("https://allowed.example","https://denied.example"):
          got=p.both("OPTIONS","/capture",headers={"origin":origin,"access-control-request-method":"POST"})
          sig=lambda x:(x["status"],x["headers"].get("access-control-allow-origin"),x["headers"].get("vary"))
          self.check("CORS "+origin,sig(got["ts"])==sig(got["rs"]),f"{sig(got['ts'])} != {sig(got['rs'])}")
        seed={"data":[{"sessionKey":"seed-e2e","sessionId":"seed-e2e",
          "conversations":[[{"role":"user","content":"种子偏好是蓝色","timestamp":1893456001000},
                            {"role":"assistant","content":"已记录","timestamp":1893456001001}]]}],
          "config_override":{"pipeline":{"everyNConversations":1,"l1IdleTimeoutSeconds":"invalid"}}}
        self.equal("seed successful replay",p.both("POST","/seed",seed,timeout=30))
        self.equal("seed output is isolated from live store",p.both("POST","/search/conversations",
          {"query":"种子偏好蓝色","session_key":"seed-e2e"}))
        self.save_trace("seed-mock-requests",self.mock("GET","/__log"))
        texts=["你好🌍 café e\u0301 🧪", "大"*(256*1024)]
        for i,text in enumerate(texts): self.equal(f"unicode/large {i}",p.both("POST","/capture",capture_payload("unicode",i,text),timeout=30))
    def slow_capture(self):
      with self.pair("slow-capture") as p:
        self.mock("POST","/__reset"); self.mock("POST","/__control",{"delayMs":3000})
        got=p.both("POST","/capture",capture_payload("slow",1),timeout=12)
        self.save_trace("slow-capture", {k:{"status":v["status"],"body":v["body"],"elapsed":v["elapsed"]} for k,v in got.items()})
        self.equal("slow LLM capture response",got)
        for k in ("ts","rs"): self.check(f"{k} capture is non-blocking",got[k]["elapsed"]<1.0,f"elapsed={got[k]['elapsed']:.3f}s")
    def scheduling(self):
      override='''  pipeline:\n    everyNConversations: 5\n    enableWarmup: true\n    l1IdleTimeoutSeconds: 30\n    l2DelayAfterL1Seconds: 0\n    l2MinIntervalSeconds: 0\n    l2MaxIntervalSeconds: 60\n'''
      with self.pair("scheduling",override) as p:
        self.mock("POST","/__reset")
        for n in range(1,13):
          self.equal(f"warmup capture {n}",p.both("POST","/capture",capture_payload("warm",n)))
          if n in (1,3,7,12):
            milestone=(1,3,7,12).index(n)+1
            self.poll_mock(f"warmup L1 at cumulative turn {n}",lambda events,m=milestone: all(
              sum(x["task"]=="l1" and x["completed_ms"] is not None
                  for x in self.provider_events(events,k))>=m for k in ("ts","rs")))
        expected_completed={"l1":4,"dedup":3,"l2":4,"l3":4}
        log=self.sample_mock_until("fourth L1 downstream dedup/L2/L3 trace completed",lambda events: all(
          all(sum(x["task"]==task and x["completed_ms"] is not None
                  for x in self.provider_events(events,k))>=count
              for task,count in expected_completed.items()) for k in ("ts","rs")),timeout=12)
        self.save_trace("warmup-l2-l3-mock-requests",log)
        import re
        sequences={}; trigger_turns={}; task_counts={}
        expected_counts=Counter(expected_completed)
        for k in ("ts","rs"):
          events=self.provider_events(log,k)
          sequences[k]=[(x["task"],x["status"]) for x in events]
          trigger_turns[k]=[max(map(int,re.findall(r"turn (\d+)",json.dumps(x["body"]))))
                            for x in events if x["task"]=="l1"]
          tasks=[task for task,_ in sequences[k]]
          task_counts[k]=Counter(tasks)
          l1_positions=[i for i,task in enumerate(tasks) if task=="l1"]
          dedup_positions=[i for i,task in enumerate(tasks) if task=="dedup"]
          l2_positions=[i for i,task in enumerate(tasks) if task=="l2"]
          l3_positions=[i for i,task in enumerate(tasks) if task=="l3"]
          self.check(f"{k} warmup L1 cumulative turns are exactly 1/3/7/12",
                     trigger_turns[k]==[1,3,7,12],str(trigger_turns[k]))
          self.check(f"{k} exact successful L1/dedup/L2/L3 counts",
                     task_counts[k]==expected_counts and all(status==200 for _,status in sequences[k]),
                     str(sequences[k]))
          self.check(f"{k} each dedup follows its corresponding successful L1",
                     len(l1_positions)==4 and len(dedup_positions)==3 and
                     all(dedup_positions[i]>l1_positions[i+1] for i in range(3)),str(tasks))
          self.check(f"{k} L2 follows at least the first L1",
                     len(l2_positions)==4 and l2_positions[0]>l1_positions[0],str(tasks))
          self.check(f"{k} each L3 follows its corresponding L2",
                     len(l3_positions)==4 and all(l3_positions[i]>l2_positions[i] for i in range(4)),str(tasks))
          self.check(f"{k} has no stray mock tasks",set(tasks)<={"l1","dedup","l2","l3"},str(tasks))
        self.check("warmup and L2/L3 causal trace matches oracle",
                   task_counts["ts"]==task_counts["rs"] and trigger_turns["ts"]==trigger_turns["rs"] and
                   [status for _,status in sequences["ts"]]==[status for _,status in sequences["rs"]],
                   f"sequence={sequences} counts={task_counts} triggers={trigger_turns}")
        self.equal("session/end flush",p.both("POST","/capture",capture_payload("flush",1)))
        self.equal("session/end response",p.both("POST","/session/end",{"session_key":"flush"},timeout=15))
    def idle_retry_concurrency_restart(self):
      override='''  pipeline:\n    everyNConversations: 100\n    enableWarmup: false\n    l1IdleTimeoutSeconds: 1\n    l2DelayAfterL1Seconds: 60\n    l2MinIntervalSeconds: 0\n    l2MaxIntervalSeconds: 60\n'''
      with self.pair("recovery",override) as p:
        self.mock("POST","/__reset"); self.mock("POST","/__control",{"failNextPerKey":1,"failStatus":503})
        # Interleaved sessions expose accidental global scheduler state.
        for n in range(4): self.equal(f"concurrent session {n}",p.both("POST","/capture",capture_payload(f"s{n%2}",n)))
        log=self.sample_mock_until("idle retry successful L1 post-dedup completed",lambda events: all(
          sum(x["task"]=="l1" and x["completed_ms"] is not None
              for x in self.provider_events(events,k))>=3 and
          sum(x["task"]=="dedup" and x["completed_ms"] is not None
              for x in self.provider_events(events,k))>=1 for k in ("ts","rs")),timeout=10)
        self.save_trace("idle-retry-concurrency-mock-requests",log)
        sig=lambda key:[(x["task"],x["status"]) for x in self.provider_events(log,key,{"l1","dedup"})]
        for k in ("ts","rs"):
          counts=Counter(task for task,_ in sig(k)); statuses=[status for _,status in sig(k)]
          self.check(f"{k} exact completed idle retry trace",
                     counts==Counter({"l1":3,"dedup":1}) and statuses.count(503)==1 and
                     statuses.count(200)==3 and all(status is not None for status in statuses),str(sig(k)))
        self.check("idle debounce and retry L1/dedup trace",sig("ts")==sig("rs"),f"TS={sig('ts')} RS={sig('rs')}")
        before={k:durable_snapshot(p.data[k]) for k in ("ts","rs")}
        p.stop(); p.start()
        for n in range(2): self.equal(f"post-restart search {n}",p.both("POST","/search/conversations",{"query":"parity","session_key":f"s{n}"}))
        p.stop(); after={k:durable_snapshot(p.data[k]) for k in ("ts","rs")}
        self.save_trace("durable-ts",after["ts"]); self.save_trace("durable-rs",after["rs"])
        self.check("TS/Rust durable SQLite JSONL checkpoint parity",after["ts"]==after["rs"],"see durable-ts.json and durable-rs.json")
        for k in ("ts","rs"): self.check(f"{k} restart preserves state",bool(after[k]["sqlite"]),"SQLite snapshot empty")
    def run(self):
      for name, scenario in (("contract", self.contract), ("slow_capture", self.slow_capture),
                             ("scheduling", self.scheduling),
                             ("idle_retry_concurrency_restart", self.idle_retry_concurrency_restart)):
        try: scenario()
        except Exception as error:
          self.failures.append(f"{name} raised {type(error).__name__}: {error}")
          print("FAIL",name,"raised",repr(error))
      report=self.work/"DIFFERENTIAL_REPORT.json"
      report.write_text(json.dumps({"oracle":ORACLE_COMMIT,"failures":self.failures},ensure_ascii=False,indent=2))
      print("report:",report)
      if self.failures:
        print("\n".join(" - "+x for x in self.failures)); raise SystemExit(1)
      print("TypeScript/Rust real-process differential passed")

def main():
    ap=argparse.ArgumentParser(); ap.add_argument("--repo",required=True); ap.add_argument("--baseline",required=True); ap.add_argument("--keep",action="store_true")
    args=ap.parse_args(); work=tempfile.mkdtemp(prefix="aeon-memory-http-diff-"); mock_port=free_port()
    log=open(pathlib.Path(work)/"mock.log","wb")
    mock=subprocess.Popen(["node",str(pathlib.Path(args.repo)/"scripts/mock-openai.mjs"),str(mock_port)],stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
    failed=False
    try:
      deadline=time.time()+5
      while time.time()<deadline:
        try:
          if request(f"http://127.0.0.1:{mock_port}","GET","/__log",timeout=.5)["status"]==200:break
        except Exception:time.sleep(.05)
      Suite(args,work,mock_port).run()
    except BaseException:
      failed=True; raise
    finally:
      if mock.poll() is None:mock.terminate(); mock.wait(3)
      log.close()
      if args.keep or failed: print("kept:",work)
      else: shutil.rmtree(work,ignore_errors=True)
if __name__ == "__main__": main()
