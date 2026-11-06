#!/usr/bin/env python3
"""Isolated core/TUI contract checks. Never reads or writes the default session."""
import json
import base64
import os
from pathlib import Path
import shlex
import subprocess
import tempfile
import time

BIN = Path(__file__).resolve().parents[1] / "target/debug/hc-tui"
ROOT = Path(tempfile.mkdtemp(prefix="hctui-parity-"))
SESSION = f"parity-{os.getpid()}"
ENV = dict(os.environ, HC_SESSION=SESSION, HC_TUI_GRAPHICS="blocks")
for key, folder in [("XDG_RUNTIME_DIR", "run"), ("XDG_STATE_HOME", "state"), ("XDG_CONFIG_HOME", "config"), ("XDG_CACHE_HOME", "cache")]:
    path = ROOT / folder
    path.mkdir(mode=0o700)
    ENV[key] = str(path)
CONFIG = ROOT / "test.toml"
CONFIG.write_text('[terminal]\ndefault_shell = "/bin/sh"\n[session]\nrestore = true\n[ui]\nconfirm_close = false\n')


def hc(*args):
    return subprocess.check_output(["hc", *args], env=ENV, text=True)


def rpc(method, params):
    # A separate line-mode connection, just as a GUI client would use.
    process = subprocess.Popen(["hc", "connect", "--session", SESSION], env=ENV, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    try:
        for ident, name, arguments in [(1, "hello", {"generation": 1}), (2, method, params)]:
            process.stdin.write(json.dumps({"jsonrpc": "2.0", "id": ident, "method": name, "params": arguments}) + "\n")
            process.stdin.flush()
            while True:
                line = process.stdout.readline()
                if not line:
                    raise RuntimeError("core disconnected")
                reply = json.loads(line)
                if reply.get("id") == ident:
                    if "error" in reply:
                        raise RuntimeError(reply["error"])
                    break
        return reply["result"]
    finally:
        process.terminate()
        process.communicate(timeout=5)


def tui(*args):
    raw = subprocess.check_output([str(BIN), *args, "--session", SESSION, "--config", str(CONFIG)], env=ENV, text=True, stderr=subprocess.PIPE)
    return json.loads(raw)


def do(name, args=None):
    return tui("do", name, json.dumps(args or {}))


def wait(label, predicate, timeout=12):
    deadline = time.monotonic() + timeout
    error = None
    while time.monotonic() < deadline:
        try:
            value = predicate()
            if value:
                print("ok  ", label, flush=True)
                return value
        except (OSError, ValueError, subprocess.CalledProcessError, RuntimeError) as failure:
            error = failure
        time.sleep(0.05)
    raise AssertionError(f"{label} timed out: {error}")


def space(ident, terminal):
    return {"id": ident, "name": "same", "folder": "/tmp", "custom": "preserve", "tabs": [{"id": "tb" + ident, "name": "", "root": {"id": "tb" + ident, "type": "row", "look": {"gap": 2}, "children": [{"id": "p" + ident, "type": "block", "content": {"terminal": terminal}}]}}]}


try:
    one = json.loads(hc("run", "--bg", "--json", "--", "/bin/sh"))["address"]
    two = json.loads(hc("run", "--bg", "--json", "--", "/bin/sh"))["address"]
    remote_session=SESSION+"-remote"
    hc("machine", "add", "--name", "peer", "--session", remote_session, "--via", "sh -c", "--json")
    wait("second core machine connects",lambda:any(m["name"]=="peer" and m["state"]=="connected" for m in json.loads(hc("machine","ls","--json"))["machines"]))
    remote_terminal=rpc("terminal.run",{"on":"peer","argv":["/bin/sh"],"cwd":"/tmp"})["address"]
    remote_local=remote_terminal.split("/",1)[-1]
    rpc("doc.set",{"name":"herdr-app","on":"peer","version":0,"value":{"spaces":[space("remote-space",remote_local)]}})
    gui = {"spaces": [space("spa", one), space("spb", two)]}
    rpc("doc.set", {"name": "herdr-app", "version": 0, "value": gui})
    subprocess.check_call(["tmux", "-L", SESSION, "new-session", "-d", "-s", "tui", "-x", "110", "-y", "32", shlex.join(["env", *[f"{k}={v}" for k, v in ENV.items() if k in {"HC_SESSION", "HC_TUI_GRAPHICS", "XDG_RUNTIME_DIR", "XDG_STATE_HOME", "XDG_CONFIG_HOME", "XDG_CACHE_HOME"}], str(BIN), "--session", SESSION, "--config", str(CONFIG)])])
    wait("imports both same-named stable space ids", lambda: {"spa", "spb"}.issubset({w.get("origin") for w in tui("ls")["state"]["workspaces"]}))
    wait("imports second core layout with machine-qualified ids",lambda:any(w.get("origin")=="peer/remote-space" for w in tui("ls")["state"]["workspaces"]))
    do("space.select",{"id":"peer/remote-space"})
    do("tab.new",{"name":"remote-child"})
    wait("new remote tab starts on the same machine",lambda:any(p["terminal"] and p["terminal"].startswith("peer/") for w in tui("ls")["state"]["workspaces"] if w.get("origin")=="peer/remote-space" for t in w["tabs"] if t["label"]=="remote-child" for p in [tui("ls")["state"]["panes"][str(t["focus"])]]))
    # A remote Pi mapping must use file.read on the terminal's core machine.
    transcript = str(ROOT / "remote-pi.jsonl")
    message = {"type":"message","id":"a","parentId":None,"message":{"role":"user","content":"REMOTE-PI-CHAT-PROOF"}}
    payload = json.dumps(message)+"\n"
    rpc("file.write",{"on":"peer","path":transcript,"data":base64.b64encode(payload.encode()).decode()})
    rpc("terminal.label",{"terminal":remote_terminal,"labels":{"session":transcript}})
    wait("remote session label reaches TUI",lambda:any(t["address"]==remote_terminal and t.get("session")==transcript for t in tui("ls")["terminals"]))
    do("pane.focus",{"terminal":remote_terminal})
    def screen():
        return subprocess.check_output(["tmux","-L",SESSION,"capture-pane","-p","-t","tui"],text=True)
    wait("remote pane attached",lambda:"starting" not in screen())
    subprocess.check_call(["tmux","-L",SESSION,"send-keys","-t","tui","C-b","A"])
    wait("remote Pi transcript visible",lambda:"REMOTE-PI-CHAT-PROOF" in screen())
    extra = {"type":"message","id":"b","parentId":"a","message":{"role":"assistant","content":[{"type":"text","text":"REMOTE-PI-APPEND-PROOF"}]}}
    payload += json.dumps(extra)+"\n"
    rpc("file.write",{"on":"peer","path":transcript,"data":base64.b64encode(payload.encode()).decode()})
    wait("remote Pi incremental append visible",lambda:"REMOTE-PI-APPEND-PROOF" in screen())
    rpc("terminal.label",{"terminal":remote_terminal,"labels":{"session":""}})
    wait("unmapped remote never displays stale local transcript",lambda:"远端会话未映射" in screen() and "REMOTE-PI-CHAT-PROOF" not in screen())
    subprocess.check_call(["tmux","-L",SESSION,"send-keys","-t","tui","Escape"])
    # Shared labels travel both directions; plain shells still remain out of Agents.
    rpc("terminal.label",{"terminal":one,"labels":{"herdr.pinned":"1","herdr.seen":"7"}})
    wait("GUI pinned/seen labels reach TUI",lambda:any(t["address"]==one and t.get("pinned") and t.get("seen_turn")==7 for t in tui("ls")["terminals"]))
    do("terminal.pin",{"terminal":one,"pinned":False})
    wait("TUI unpin reaches shared terminal labels",lambda:json.loads(hc("get",one)).get("labels",{}).get("herdr.pinned","")!="1")
    do("space.rename", {"id": "spa", "name": "renamed"})
    result = wait("rename reaches GUI document through CAS", lambda: (lambda d: d if any(w["id"] == "spa" and w["name"] == "renamed" for w in d["value"]["spaces"]) else None)(rpc("doc.get", {"name": "herdr-app"})))
    preserved = next(w for w in result["value"]["spaces"] if w["id"] == "spa")
    assert preserved["custom"] == "preserve"
    assert preserved["tabs"][0]["root"]["look"] == {"gap": 2}
    print("ok   preserves GUI root styling and unknown properties", flush=True)
    # Simulate another GUI writer moving/renaming a space.
    latest = rpc("doc.get", {"name": "herdr-app"})
    latest["value"]["spaces"] = list(reversed(latest["value"]["spaces"]))
    next(w for w in latest["value"]["spaces"] if w["id"] == "spb")["name"] = "from-gui"
    rpc("doc.set", {"name": "herdr-app", "version": latest["version"], "value": latest["value"]})
    wait("GUI edits return to TUI", lambda: any(w.get("origin") == "spb" and w["label"] == "from-gui" for w in tui("ls")["state"]["workspaces"]))
    do("space.select", {"id": "spa"})
    do("tab.settle", {"id": "tbspa"})
    wait("settle is view state", lambda: any(t["origin"] == "tbspa" and t["settled"] for w in tui("ls")["state"]["workspaces"] for t in w["tabs"]))
    do("tab.unsettle", {"id": "tbspa"})
    do("terminal.park", {"terminal": one})
    wait("park keeps program alive and removes its view", lambda: json.loads(hc("get", one))["state"] == "live" and not any(p["terminal"] == one for p in tui("ls")["state"]["panes"].values()))
    assert not json.loads(hc("get", one))["keep"]
    do("terminal.keep", {"terminal": one})
    wait("keep restores the view", lambda: any(p["terminal"] == one for p in tui("ls")["state"]["panes"].values()))
    do("pane.close")
    wait("close ends its program", lambda: json.loads(hc("get", one))["state"] == "finished")
    # Quitting the app is a detach, distinct from closing a pane.
    do("app.quit")
    wait("quit leaves other programs alive", lambda: json.loads(hc("get", two))["state"] == "live")
    print("herdr parity smoke: passed; artifacts:", ROOT)
finally:
    subprocess.run(["tmux", "-L", SESSION, "kill-server"], capture_output=True)
    subprocess.run(["hc", "server", "stop"], env=ENV, capture_output=True)
    remote_env=dict(ENV,HC_SESSION=SESSION+"-remote")
    subprocess.run(["hc", "server", "stop"], env=remote_env, capture_output=True)
