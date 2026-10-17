import socket
import json
import sys

def rpc(cmd, args=None):
    if args is None:
        args = {}
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.connect("/tmp/arb-dev.sock")
    s.settimeout(20.0)
    msg = json.dumps({"cmd": cmd, "args": args}) + "\n"
    s.sendall(msg.encode())
    resp = s.makefile().readline()
    s.close()
    return json.loads(resp)

if __name__ == "__main__":
    if len(sys.argv) > 1:
        cmd = sys.argv[1]
        args = json.loads(sys.argv[2]) if len(sys.argv) > 2 else {}
        print(json.dumps(rpc(cmd, args), indent=2))
