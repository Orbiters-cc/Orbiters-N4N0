"""Small authenticated MCP client for evidence scripts; credentials are never logged."""
import importlib.util
import json
from pathlib import Path
import time

repo=Path(__file__).parents[2]
spec=importlib.util.spec_from_file_location('bridge_live',repo/'scripts/bridge-live.py')
module=importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)

class Bench:
    def __init__(self):self.bridge=module.Bridge()
    def call(self, **args):
        began=time.time()
        result=self.bridge.call('vr_sandbox',args)['result']
        data=result.get('structuredContent')
        if data is None:data=json.loads(result['content'][0]['text'])
        evidence=Path.home()/'.orbiters-build/vr-feedback-1111/sandbox-camera/events.jsonl'
        with evidence.open('a',encoding='utf-8') as out:out.write(json.dumps({'began':began,'ended':time.time(),'args':args,'response':data,'error':result.get('isError',False)})+'\n')
        print(json.dumps(data),flush=True)
        if result.get('isError'):raise RuntimeError(data)
        return data
