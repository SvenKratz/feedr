#!/usr/bin/env python3
"""Exercise real broker/CLI/MCP processes in an isolated store."""
import concurrent.futures
import json
import os
import pathlib
import selectors
import signal
import socket
import subprocess
import sys
import tempfile
import time

binary = str(pathlib.Path(sys.argv[1]).resolve())
env = {k: v for k, v in os.environ.items() if not k.startswith(("HERDR_", "FEEDR_"))}
children = []

with tempfile.TemporaryDirectory(prefix="feedr-smoke-") as root:
    base = [binary, "--data-dir", root]
    log = open(pathlib.Path(root) / "service-test.log", "w")

    def launch_broker():
        child = subprocess.Popen(base + ["serve"], env=env, stdout=log, stderr=log)
        children.append(child)
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            try:
                with socket.socket(socket.AF_UNIX) as stream:
                    stream.connect(root + "/feedr.sock")
                    stream.sendall(b'{"version":1,"method":"ping","params":{}}\n')
                    if json.loads(stream.makefile().readline())["ok"]:
                        return child
            except (OSError, ValueError):
                time.sleep(0.05)
        raise RuntimeError("Broker failed to start: " + pathlib.Path(root, "service-test.log").read_text())

    def cli(*args, input=None, expect_ok=True):
        result = subprocess.run(base + list(args), input=input, capture_output=True, text=True, env=env, timeout=12)
        if expect_ok:
            assert result.returncode == 0, result.stderr
            return json.loads(result.stdout)
        assert result.returncode != 0, "Expected a failing request"
        return result.stderr

    def stop(child):
        child.terminate()
        child.wait(timeout=5)

    try:
        broker = launch_broker()
        artifact = pathlib.Path(root) / "a report with spaces.md"
        artifact.write_text("# Live smoke test\n\nA persisted artifact.\n")
        one = cli("post", "--title", "Smoke report", "--idempotency-key", "smoke-report", str(artifact))
        two = cli("post", "--title", "Smoke report", "--idempotency-key", "smoke-report", str(artifact))
        assert one["id"] == two["id"]
        assert one["resources"][0]["target"] == str(artifact.resolve())

        def publish(i):
            return cli("post", "--title", f"Concurrent {i}", "--idempotency-key", f"concurrent-{i}", str(artifact))

        with concurrent.futures.ThreadPoolExecutor(max_workers=6) as pool:
            posts = list(pool.map(publish, range(12)))
        assert len({p["id"] for p in posts}) == 12
        assert cli("list")["total"] == 13

        updated = cli("update", "--input", "-", input=json.dumps({"id":one["id"],"expected_revision":1,"pinned":True,"body":"Revised"}))
        assert updated["revision"] == 2 and updated["pinned"]
        assert "Revision conflict" in cli("update", "--input", "-", input=json.dumps({"id":one["id"],"expected_revision":1,"body":"Wrong"}),expect_ok=False)
        stop(broker)
        broker = launch_broker()
        assert cli("get", one["id"])["pinned"]
        assert cli("post", "--title", "Smoke report", "--idempotency-key", "smoke-report", str(artifact))["id"] == one["id"]

        mcp = subprocess.Popen(base + ["mcp"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log, text=True, env=env, bufsize=1)
        children.append(mcp)
        selector = selectors.DefaultSelector()
        selector.register(mcp.stdout, selectors.EVENT_READ)

        def rpc(message, response=True):
            mcp.stdin.write(json.dumps(message) + "\n")
            mcp.stdin.flush()
            if not response:
                return None
            deadline = time.monotonic() + 8
            while time.monotonic() < deadline:
                assert selector.select(timeout=8), "MCP response timed out"
                line = mcp.stdout.readline()
                assert line, "MCP server closed stdout"
                value = json.loads(line)
                if value.get("id") == message["id"]:
                    assert "error" not in value, value
                    return value["result"]
            raise RuntimeError("No matching MCP response")

        initialized = rpc({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"feedr-smoke","version":"1"}}})
        assert "tools" in initialized["capabilities"]
        assert initialized["serverInfo"]["name"] == "feedr"
        rpc({"jsonrpc":"2.0","method":"notifications/initialized"},False)
        tools = rpc({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}})
        assert {t["name"] for t in tools["tools"]} == {"feedr_post","feedr_update","feedr_get","feedr_list","feedr_sessions"}
        assert next(t for t in tools["tools"] if t["name"] == "feedr_list")["annotations"]["readOnlyHint"]
        result = rpc({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"feedr_post","arguments":{"title":"MCP result","resources":[{"uri":str(artifact)}],"idempotency_key":"mcp-result"}}})
        assert not result.get("isError",False), result
        posted = json.loads(result["content"][0]["text"])
        assert cli("get",posted["id"])["title"] == "MCP result"
        listed = rpc({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"feedr_list","arguments":{"search":"MCP result"}}})
        assert json.loads(listed["content"][0]["text"])["total"] == 1
        bad = rpc({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"feedr_update","arguments":{"id":posted["id"],"expected_revision":99,"body":"conflict"}}})
        assert bad.get("isError"), bad
        assert cli("list")["total"] == 14
        # Only the synthetic manual session is opted in; never inspect the
        # user's actual herdr panes while running a process smoke test.
        session = one["session_id"]
        assert not cli("discovery", "status")["enabled"]
        cli("discovery", "ingest", "--session", session, input="./mention.md", expect_ok=False)
        cli("discovery", "enable", "--session", session)
        mention_file = pathlib.Path(root, "mentioned report.md")
        mention_file.write_text("# Discovered\n")
        text = f"Ready: [report](<{mention_file}>)\n"
        found = cli("discovery", "ingest", "--session", session, input=text)
        assert len(found["created"]) == 1
        mention = cli("get", found["created"][0])
        assert mention["discovery"]["source"] == "hook"
        assert cli("discovery", "ingest", "--session", session, input=text)["matched"] == found["created"]
        dismissed = cli("update", "--input", "-", input=json.dumps({"id":mention["id"],"expected_revision":mention["revision"],"archived":True}))
        stop(broker)
        broker = launch_broker()
        assert cli("discovery", "status")["enabled"]
        assert cli("discovery", "ingest", "--session", session, input=text)["created"] == []
        assert cli("get",dismissed["id"])["archived"]
        log_path = pathlib.Path(root,"assistant.jsonl")
        log_path.write_text("")
        cli("discovery", "watch-log", "--session", session, "--format", "claude", str(log_path))
        log_url = "https://example.invalid/feedr-discovery-smoke"
        line = json.dumps({"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":f"Preview {log_url}"}]}})
        log_path.write_text(line[:len(line)//2])
        time.sleep(2.2)
        assert cli("list", "--search", log_url)["total"] == 0
        with log_path.open("a") as log_file:
            log_file.write(line[len(line)//2:]+"\n")
        deadline = time.monotonic()+10
        while cli("list", "--search", log_url)["total"] != 1:
            assert time.monotonic()<deadline, "Completed log mention was not discovered"
            time.sleep(0.1)
        cli("discovery", "disable")
        assert cli("stop")["stopping"]
        broker.wait(timeout=12)
        broker = launch_broker()
        assert cli("get", one["id"])["pinned"]
        print("PASS: opt-in discovery, durable deduplication/dismissal, session binding and partial JSONL tailing")
        print("PASS: graceful service shutdown preserves acknowledged posts across restart")
        print("PASS: concurrent CLI posts, deduplication, revision conflicts, restart recovery, MCP handshake and tool calls")
    finally:
        for child in reversed(children):
            if child.poll() is None:
                child.terminate()
                try:
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait(timeout=5)
        log.close()

subprocess.run([sys.executable, str(pathlib.Path(__file__).with_name("agent-smoke.py")), binary], check=True)
