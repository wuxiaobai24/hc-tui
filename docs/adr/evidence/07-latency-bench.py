#!/usr/bin/env python3
"""E7: winless `hc-tui do` round trip and persistent-connection doc.get."""
import json, math, os, subprocess, sys, time

BIN = sys.argv[1]
N = int(sys.argv[2]) if len(sys.argv) > 2 else 25
SESSION = os.environ["HC_SESSION"]


def percentile(samples, p):
    """Nearest-rank: index = ceil(p/100 * n) - 1."""
    s = sorted(samples)
    return s[max(0, math.ceil(p / 100 * len(s)) - 1)]


class Conn:
    def __init__(self):
        self.proc = subprocess.Popen(
            ["hc", "connect", "--session", SESSION],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
        self.next_id = 0

    def call(self, method, params):
        self.next_id += 1
        request = {"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params}
        self.proc.stdin.write(json.dumps(request) + "\n")
        self.proc.stdin.flush()
        while True:
            reply = json.loads(self.proc.stdout.readline())
            if reply.get("id") == self.next_id:
                return reply

    def close(self):
        self.proc.stdin.close()
        self.proc.wait()


conn = Conn()
conn.call("hello", {"generation": 1})
doc = conn.call("doc.get", {"name": "hctui-layout-v1"})["result"]
conn.close()
spaces = [w for w in doc["value"]["workspaces"] if w["label"] == "bench"]
space_id = spaces[0]["origin"]

args = json.dumps({"id": space_id, "name": "bench"})
for _ in range(3):
    subprocess.run([BIN, "do", "space.rename", args], capture_output=True, check=True)

do_samples = []
for _ in range(N):
    started = time.perf_counter()
    done = subprocess.run([BIN, "do", "space.rename", args], capture_output=True, text=True)
    do_samples.append((time.perf_counter() - started) * 1000)
    assert done.returncode == 0, done.stdout + done.stderr
    assert '"accepted":true' in done.stdout.replace(" ", ""), done.stdout

conn = Conn()
conn.call("hello", {"generation": 1})
for _ in range(3):
    conn.call("doc.get", {"name": "hctui-layout-v1"})
get_samples = []
for _ in range(N):
    started = time.perf_counter()
    conn.call("doc.get", {"name": "hctui-layout-v1"})
    get_samples.append((time.perf_counter() - started) * 1000)
conn.close()

print(f"session: {SESSION}")
print(f"space:   {space_id}")
print(f"n = {N}, warmup = 3, percentile = nearest-rank")
print("hc-tui do (winless, full process) ms:",
      [round(x, 2) for x in do_samples])
print("  p50 = %.2f ms  p90 = %.2f ms" % (percentile(do_samples, 50), percentile(do_samples, 90)))
print("doc.get (one persistent connection) ms:",
      [round(x, 3) for x in get_samples])
print("  p50 = %.3f ms  p90 = %.3f ms" % (percentile(get_samples, 50), percentile(get_samples, 90)))
