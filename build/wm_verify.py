#!/usr/bin/env python3
"""SP3 compositor WINDOW MANAGER runtime verifier.

Boots build/comptest.iso headless under QEMU+KVM with QMP, then drives the PS/2
(RELATIVE) mouse through four window-manager transitions, capturing a screendump
after each:

  1. wm-before.png       baseline: two decorated windows (A blue/focused bar,
                         B grey bar), white title text, red [X] each, B over A.
  2. wm-after-drag.png   DRAG: grab A's title bar, move it right+down; A moves,
                         B unchanged.
  3. wm-after-raise.png  RAISE: click an A-only point so A raises above B;
                         A bar blue + occludes B; serial shows WM-FOCUS 0.
  4. wm-after-close.png   CLOSE: click B's [X]; B disappears entirely, no ghost,
                         only A remains.

Then quits. Primary proof = serial `WM-FOCUS <idx>` lines + the start marker
`[wm] compositor SP3: window manager (2 windows)`. Visual proof = the 4 PNGs
(inspected by the controller).

Mouse model: PS/2 is RELATIVE. gfx centres the cursor at boot ~(640,400) on the
1280x800 framebuffer. We TRACK a virtual cursor and emit REL deltas in steps
<= ~55px so PS/2 accel does not overshoot. We clamp the virtual cursor to the
screen (gfx does too) so tracking stays in sync with the kernel's cursor.

Window geometry (from Compositor::new, TITLE_H=28):
  A id0 "reactor A": surface (60, 88, 320, 240) -> footprint (60, 60, 320, 268)
                     title y[60,88) x[60,380); [X] x[352,380) y[60,88)
  B id1 "reactor B": surface (300,198,320,240) -> footprint (300,170,320,268)
                     title y[170,198) x[300,620); [X] x[592,620) y[170,198)
  z-order: wins[0]=A bottom, wins[1]=B top. Overlap x[300,380) y[198,328).
"""
import socket, json, time, sys, os

ROOT = "/mnt/e/MinimalOS/BasicOperatingSystem"
SOCK = "/tmp/qmp.sock"
SERIAL = ROOT + "/build/wm-serial.log"
SHOT_BEFORE = ROOT + "/build/wm-before.png"
SHOT_DRAG = ROOT + "/build/wm-after-drag.png"
SHOT_RAISE = ROOT + "/build/wm-after-raise.png"
SHOT_CLOSE = ROOT + "/build/wm-after-close.png"

SCREEN_W, SCREEN_H = 1280, 800
STEP = 55  # max px per REL injection so PS/2 accel does not overshoot

# Virtual cursor, starts where gfx centres it at boot.
cur = [SCREEN_W // 2, SCREEN_H // 2]  # ~(640, 400)


def connect():
    for _ in range(120):
        try:
            s = socket.socket(socket.AF_UNIX)
            s.connect(SOCK)
            return s
        except OSError:
            time.sleep(0.5)
    print("FAIL: QMP timeout")
    sys.exit(1)


def main():
    s = connect()
    f = s.makefile("rw")

    def cmd(o):
        f.write(json.dumps(o) + "\n")
        f.flush()
        return json.loads(f.readline())

    def shot(path):
        cmd({"execute": "screendump", "arguments": {"filename": path, "format": "png"}})

    def rel(dx, dy):
        evs = []
        if dx:
            evs.append({"type": "rel", "data": {"axis": "x", "value": dx}})
        if dy:
            evs.append({"type": "rel", "data": {"axis": "y", "value": dy}})
        if evs:
            cmd({"execute": "input-send-event", "arguments": {"events": evs}})

    def move_to(tx, ty, settle=0.06):
        """Walk the virtual cursor to (tx,ty) in REL steps <= STEP each."""
        tx = max(0, min(SCREEN_W - 1, tx))
        ty = max(0, min(SCREEN_H - 1, ty))
        while cur[0] != tx or cur[1] != ty:
            dx = max(-STEP, min(STEP, tx - cur[0]))
            dy = max(-STEP, min(STEP, ty - cur[1]))
            rel(dx, dy)
            cur[0] += dx
            cur[1] += dy
            time.sleep(settle)
        time.sleep(0.2)

    def btn(down):
        cmd({"execute": "input-send-event", "arguments": {"events": [
            {"type": "btn", "data": {"button": "left", "down": down}}]}})

    json.loads(f.readline())  # QMP greeting
    cmd({"execute": "qmp_capabilities"})

    # ---- 1. BASELINE ------------------------------------------------------
    print("waiting for boot + first render (14s)...")
    time.sleep(14)
    shot(SHOT_BEFORE)
    print("STEP 1 BEFORE  cursor=%s  -> %s" % (tuple(cur), SHOT_BEFORE))
    print("  EXPECT: two windows; A blue bar (focused) + B grey bar; white title")
    print("          text; red [X] each; B overlaps A.")

    # ---- 2. DRAG A's title bar --------------------------------------------
    # A title bar y[60,88) x[60,380); [X] x[352,380). Target left of [X].
    print("\nSTEP 2 DRAG: grab A's title bar at (200,74), drag to ~(520,360)")
    move_to(200, 74)
    print("  cursor on A title bar: %s" % (tuple(cur),))
    btn(True)
    time.sleep(0.2)
    # Drag right+down in several short steps so the window tracks smoothly.
    for (tx, ty) in [(280, 150), (360, 230), (440, 300), (520, 360)]:
        move_to(tx, ty, settle=0.08)
    time.sleep(0.3)
    btn(False)
    time.sleep(1.0)
    shot(SHOT_DRAG)
    print("  released at cursor=%s -> %s" % (tuple(cur), SHOT_DRAG))
    print("  EXPECT: A translated to new spot (footprint origin ~ cursor-grab);")
    print("          B unchanged. PASS if A moved & B still in place.")
    # After this drag, A's footprint origin = cursor - grab. grab = (200-60, 74-60)
    # = (140,14). cursor=(520,360) => footprint origin ~ (380,346) => A surface
    # ~ (380, 374, 320, 240), title bar y[346,374). (clamped on-screen)
    a_fx = cur[0] - 140
    a_fy = cur[1] - 14
    # clamp like drag_to (footprint w=320, h=268)
    a_fx = max(0, min(SCREEN_W - 320, a_fx))
    a_fy = max(0, min(SCREEN_H - 268, a_fy))
    a_sx, a_sy = a_fx, a_fy + 28
    print("  computed A footprint origin=(%d,%d) surface=(%d,%d,320,240)"
          % (a_fx, a_fy, a_sx, a_sy))

    # ---- 3. RAISE: genuine raise-from-behind ------------------------------
    # The drag (title-grab) already raised A to top + focused it. To PROVE a real
    # raise-on-click (and produce a WM-FOCUS 0 transition), first lower A by
    # focusing+raising B (click B's title bar -> WM-FOCUS 1, B now on top), then
    # click an A-ONLY point to raise A back to the front -> WM-FOCUS 0.
    #
    # B is unmoved: title bar y[170,198) x[300,620); [X] x[592,620). Click B's
    # bar left of its [X], e.g. (360,184). This raises+focuses B (and starts a
    # drag, but we release without moving, so B does NOT move).
    print("\nSTEP 3a pre-raise: click B title bar at (360,184) to put B on top "
          "(WM-FOCUS 1)")
    move_to(360, 184)
    btn(True)
    time.sleep(0.15)
    btn(False)
    time.sleep(0.6)
    # Now click an A-ONLY point so A raises above B. A footprint after drag:
    # x[a_fx, a_fx+320) y[a_fy, a_fy+268). B footprint x[300,620) y[170,438).
    # A's lower band (y > 438) and right band (x > 620) are A-only. Pick A's
    # surface lower-right area which is clear of B. Use a point near A's bottom.
    raise_x = a_fx + 250          # well right within A
    raise_y = a_fy + 240          # near A's bottom (below B's footprint y<438)
    # safety: if that y is still under B, push further down within A's footprint
    if raise_y < 438:
        raise_y = min(a_fy + 260, a_fy + 267)
    print("STEP 3b RAISE: click A-only surface point (%d,%d) to raise+focus A "
          "(WM-FOCUS 0)" % (raise_x, raise_y))
    move_to(raise_x, raise_y)
    btn(True)
    time.sleep(0.15)
    btn(False)
    time.sleep(1.0)
    shot(SHOT_RAISE)
    print("  clicked cursor=%s -> %s" % (tuple(cur), SHOT_RAISE))
    print("  EXPECT: A fully in front (occludes B in overlap), A bar blue,")
    print("          B bar grey; serial WM-FOCUS 1 then WM-FOCUS 0.")

    # ---- 4. CLOSE: click B's [X] ------------------------------------------
    # B unmoved: [X] x[592,620) y[170,198) => centre ~ (606, 184).
    print("\nSTEP 4 CLOSE: click B's [X] at (606,184)")
    move_to(606, 184)
    btn(True)
    time.sleep(0.15)
    btn(False)
    time.sleep(1.0)
    shot(SHOT_CLOSE)
    print("  clicked cursor=%s -> %s" % (tuple(cur), SHOT_CLOSE))
    print("  EXPECT: B GONE (bar+surface), no ghost; only A remains.")

    cmd({"execute": "quit"})
    try:
        s.close()
    except Exception:
        pass

    # ---- SERIAL -----------------------------------------------------------
    time.sleep(0.6)
    serial = ""
    if os.path.exists(SERIAL):
        with open(SERIAL, "rb") as fh:
            serial = fh.read().decode("utf-8", "replace")
    lines = serial.splitlines()
    start = [ln.strip() for ln in lines if "window manager" in ln or "SP3" in ln]
    focus = [ln.strip() for ln in lines if "WM-FOCUS" in ln]
    print("\n--- start marker ---")
    for ln in start:
        print(ln)
    if not start:
        print("WARN: no SP3 start marker found")
    print("\n--- WM-FOCUS serial lines ---")
    for ln in focus:
        print(ln)
    if not focus:
        print("WARN: no WM-FOCUS lines found")
    has0 = any("WM-FOCUS 0" in ln for ln in focus)
    if has0:
        print("SERIAL: PASS WM-FOCUS 0 present (A re-focused on raise)")
    else:
        print("SERIAL: NOTE WM-FOCUS 0 not found (A may have stayed focused)")
    print("\n=== screendumps: %s %s %s %s ===" %
          (SHOT_BEFORE, SHOT_DRAG, SHOT_RAISE, SHOT_CLOSE))


if __name__ == "__main__":
    main()
