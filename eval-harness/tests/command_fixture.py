"""Protocol conformance fixture, not an intelligent Agent."""
import json
import os
from pathlib import Path
import subprocess
import sys
import time

mode=sys.argv[1]
request=json.loads(sys.stdin.readline())
assert 'checks' not in request and 'answer' not in request
if mode=='bad':
    print('not-json',flush=True)
elif mode=='large':
    print('x'*70000,flush=True)
elif mode=='missing':
    print(json.dumps({'type':'text','text':'no result'}),flush=True)
elif mode=='failed':
    print('intentional failure',file=sys.stderr);sys.exit(1)
elif mode=='duplicate':
    for _ in range(2):print(json.dumps({'type':'tool','id':'same','name':'read','status':'success'}),flush=True)
elif mode=='hang':
    time.sleep(120)
elif mode=='child':
    marker=Path(request['state_dir'])/'heartbeat.txt'
    code="from pathlib import Path;import time,sys;p=Path(sys.argv[1]);\nwhile True:p.write_text(str(time.monotonic()));time.sleep(.05)"
    subprocess.Popen([sys.executable,'-u','-c',code,str(marker)])
    print(json.dumps({'type':'text','text':'child started'}),flush=True)
    time.sleep(120)
elif mode=='echo':
    state=Path(request['state_dir'])/'calls.txt'
    old=int(state.read_text()) if state.exists() else 0
    state.write_text(str(old+1))
    output=request['prompt'].removeprefix('Reply with exactly ').strip()
    print(json.dumps({'type':'text','text':output}),flush=True)
    print(json.dumps({'type':'result','protocol':1,'status':'completed','output':output,'tool_events_complete':True}),flush=True)
