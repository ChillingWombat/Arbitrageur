import socket
import json
import time

def rpc_call(cmd, args={}, timeout=30.0):
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(timeout)
    s.connect('/tmp/arb-dev.sock')
    req = json.dumps({"cmd": cmd, "args": args}) + "\n"
    s.sendall(req.encode('utf-8'))
    
    buf = ""
    start = time.time()
    while time.time() - start < timeout:
        try:
            chunk = s.recv(65536)
            if not chunk:
                break
            buf += chunk.decode('utf-8')
            if "\n" in buf:
                break
        except socket.timeout:
            break
    s.close()
    if not buf:
        return {"err": "empty response from dev rpc"}
    return json.loads(buf.strip())

if __name__ == "__main__":
    res = rpc_call("vault_status")
    print(res)
