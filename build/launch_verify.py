#!/usr/bin/env python3
"""SP5 launcher (taskbar) spawn/close runtime verifier.

Boots build/comptest.iso headless under QEMU+QMP, screenshots the steady
two-window composite + launcher taskbar (launch-0-initial), clicks launcher
button0 (react-A) to spawn a 3rd window (launch-1-spawned), clicks button2
(selfclose) which spawns then self-closes/reaps (launch-2-selfclosed), and
finally (best-effort) clicks the spawned react-A window's [X] to test the
manual close path (launch-3-xclosed), then quits.

Primary proof = serial log lines:
  - `spawn app='react-A' ... live=3`    (launcher click spawned the 3rd window)
  - `spawn app='selfclose' ...`         (button2 spawned the self-closer)
  - `reaped win_id=... (Store/Instance dropped)`  (teardown happened)

Layout (Limine fb = 1280x800): demo windows start as 2 (reactor A surface at
(60,88), reactor B at (300,198)). Launcher strip across the bottom, buttons 96px
wide:
  button0 react-A   center ~ (48, 786)
  button1 react-B   center ~ (144, 786)
  button2 selfclose center ~ (240, 786)
Cursor starts centered ~(640,400).
"""
import socket, json, time, sys, os

ROOT = "/mnt/e/MinimalOS/BasicOperatingSystem"
SOCK = "/tmp/qmp.sock"
SHOT0 = ROOT + "/build/launch-0-initial.png"
SHOT1 = ROOT + "/build/launch-1-spawned.png"
SHOT2 = ROOT + "/build/launch-2-selfclosed.png"
SHOT3 = ROOT + "/build/launch-3-xclosed.png"
SERIAL = ROOT + "/build/launch-serial.log"

# Assumed framebuffer resolution (Limine default for this VM).
FB_W, FB_H = 1280, 800
# Cursor starts centered after gfx::enter().
START_X, START_Y = FB_W // 2, FB_H // 2

# Launcher button centers (px) — y is well inside the 28px bottom strip.
BTN_Y = FB_H - 14            # ~786 on an 800-high screen
BTN0 = (48, BTN_Y)           # react-A
BTN1 = (144, BTN_Y)          # react-B
BTN2 = (240, BTN_Y)          # selfclose

# A spawned window cascades: surface ~ (40+2*28, TITLE_H+40+2*28) = (96,124);
# footprint top ~ (96,96). Its [X] is the top-right of the footprint:
#   surface w=320 -> footprint right edge ~ 96+320=416; title bar y ~ 96..124.
# [X] button is a 28px square at the right end of the title bar.
XCLOSE = (416 - 14, 96 + 14)  # ~ (402, 110)


def connect():
    for _ in range(120):
        try:
            s = socket.socket(socket.AF_UNIX)
            s.connect(SOCK)
            return s
        except OSError:
            time.sleep(0.5)
    print("FAIL: QMP timeout"); sys.exit(1)


def main():
    s = connect()
    f = s.makefile("rw")

    def cmd(o):
        f.write(json.dumps(o) + "\n"); f.flush()
        return json.loads(f.readline())

    json.loads(f.readline())  # greeting
    cmd({"execute": "qmp_capabilities"})

    # Track absolute cursor in our own model; QEMU 'rel' deltas move it.
    cur = [START_X, START_Y]

    def move_to(tx, ty):
        """Walk the relative cursor to (tx,ty) in small steps."""
        dx = tx - cur[0]
        dy = ty - cur[1]
        steps = 12
        for k in range(steps):
            # last step takes the remainder so we land exactly.
            sx = (dx - dx * k // steps) - (dx - dx * (k + 1) // steps)
            sy = (dy - dy * k // steps) - (dy - dy * (k + 1) // steps)
            cmd({"execute": "input-send-event", "arguments": {"events": [
                {"type": "rel", "data": {"axis": "x", "value": sx}},
                {"type": "rel", "data": {"axis": "y", "value": sy}}]}})
            time.sleep(0.04)
        cur[0], cur[1] = tx, ty
        time.sleep(0.3)

    def click():
        cmd({"execute": "input-send-event", "arguments": {"events": [
            {"type": "btn", "data": {"button": "left", "down": True}}]}})
        time.sleep(0.15)
        cmd({"execute": "input-send-event", "arguments": {"events": [
            {"type": "btn", "data": {"button": "left", "down": False}}]}})

    def shot(path):
        cmd({"execute": "screendump", "arguments": {"filename": path, "format": "png"}})
        print("captured:", path)

    print("waiting for boot + first render (16s)...")
    time.sleep(16)
    shot(SHOT0)

    # 1) Spawn react-A via launcher button0.
    print("clicking launcher button0 (react-A) ...")
    move_to(*BTN0)
    click()
    time.sleep(1.5)
    shot(SHOT1)

    # 2) Spawn selfclose via launcher button2; it self-closes on frame 3.
    print("clicking launcher button2 (selfclose) ...")
    move_to(*BTN2)
    click()
    time.sleep(2.0)
    shot(SHOT2)

    # 3) Best-effort: click the spawned react-A window's [X] to test manual close.
    print("clicking spawned react-A [X] (best-effort) ...")
    move_to(*XCLOSE)
    click()
    time.sleep(1.5)
    shot(SHOT3)

    cmd({"execute": "quit"})
    try:
        s.close()
    except Exception:
        pass

    # --- Assertions from serial ---------------------------------------------
    time.sleep(0.5)
    serial = ""
    if os.path.exists(SERIAL):
        with open(SERIAL, "rb") as fh:
            serial = fh.read().decode("utf-8", "replace")

    lines = serial.splitlines()
    spawn_a = [ln for ln in lines if "spawn app='react-A'" in ln and "live=3" in ln]
    spawn_sc = [ln for ln in lines if "spawn app='selfclose'" in ln]
    reaped = [ln for ln in lines if "reaped win_id=" in ln and "(Store/Instance dropped)" in ln]

    print("\n--- spawn/reap serial lines ---")
    for ln in spawn_a + spawn_sc + reaped:
        print(ln.strip())

    ok = True
    if spawn_a:
        print("SERIAL: PASS spawn react-A live=3 present")
    else:
        print("SERIAL: FAIL spawn react-A live=3 MISSING"); ok = False
    if spawn_sc:
        print("SERIAL: PASS spawn selfclose present")
    else:
        print("SERIAL: FAIL spawn selfclose MISSING"); ok = False
    if reaped:
        print("SERIAL: PASS reaped (Store/Instance dropped) present")
    else:
        print("SERIAL: FAIL reaped MISSING"); ok = False

    print("\n=== RESULT:", "PASS" if ok else "FAIL", "===")
    sys.exit(0 if ok else 2)


if __name__ == "__main__":
    main()
