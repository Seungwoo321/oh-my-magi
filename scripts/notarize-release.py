#!/usr/bin/env python3
import json
import os
import pty
import select
import signal
import subprocess
import sys
import termios
import time

mode = sys.argv[1] if len(sys.argv) > 1 else "history"
arguments = ["xcrun", "notarytool", mode]
if mode == "submit":
    if len(sys.argv) != 3:
        raise SystemExit("A local signed artifact is required.")
    arguments += [sys.argv[2], "--wait"]
elif mode != "history":
    raise SystemExit("Unsupported notarization operation.")
if os.environ.get("APPLE_KEYCHAIN_PROFILE"):
    arguments += ["--keychain-profile", os.environ["APPLE_KEYCHAIN_PROFILE"]]
elif os.environ.get("APPLE_API_KEY_PATH") and os.environ.get("APPLE_API_KEY"):
    arguments += ["--key", os.environ["APPLE_API_KEY_PATH"], "--key-id", os.environ["APPLE_API_KEY"]]
    if os.environ.get("APPLE_API_ISSUER"):
        arguments += ["--issuer", os.environ["APPLE_API_ISSUER"]]
else:
    if not all(os.environ.get(name) for name in ["APPLE_ID", "APPLE_TEAM_ID", "APPLE_PASSWORD"]):
        raise SystemExit("Notarization credentials have not been configured.")
    arguments += ["--apple-id", os.environ["APPLE_ID"], "--team-id", os.environ["APPLE_TEAM_ID"]]
arguments += ["--output-format", "json"]
master, slave = pty.openpty()
attributes = termios.tcgetattr(slave)
attributes[3] &= ~termios.ECHO
termios.tcsetattr(slave, termios.TCSANOW, attributes)
process = subprocess.Popen(arguments, stdin=slave, stdout=slave, stderr=slave, start_new_session=True)
os.close(slave)
output = bytearray()
prompt_answered = False
deadline = time.monotonic() + (1200 if mode == "submit" else 55)
timed_out = False
while time.monotonic() < deadline:
    if select.select([master], [], [], 0.2)[0]:
        try:
            chunk = os.read(master, 4096)
        except OSError:
            break
        if len(output) + len(chunk) > 1024 * 1024:
            timed_out = True
            break
        output.extend(chunk)
        if not prompt_answered and b"password" in output.lower() and os.environ.get("APPLE_PASSWORD"):
            os.write(master, os.environ["APPLE_PASSWORD"].encode() + b"\n")
            prompt_answered = True
    if process.poll() is not None and not select.select([master], [], [], 0)[0]:
        break
if process.poll() is None:
    timed_out = True
    os.killpg(process.pid, signal.SIGTERM)
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.wait()
os.close(master)
text = output.decode(errors="replace")
receipt = {"operation": mode, "exitCode": process.returncode, "timedOut": timed_out, "securePromptUsed": prompt_answered, "unauthorized401": "401" in text, "accepted": False}
try:
    document = json.loads(text[text.index("{"):].strip())
    receipt["accepted"] = process.returncode == 0 and (mode == "history" or document.get("status") == "Accepted")
    if mode == "submit":
        receipt["submissionId"] = document.get("id")
        receipt["status"] = document.get("status")
except (ValueError, KeyError):
    pass
print(json.dumps(receipt))
raise SystemExit(0 if receipt["accepted"] else 1)
