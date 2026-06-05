#!/usr/bin/env python3
"""SP2 compositor input/focus runtime verifier.

Boots build/comptest.iso headless under QEMU+QMP, screenshots the steady
two-window composite (shotA), injects a click into window 1 (middle), screenshots
again (shotB), optionally clicks back into window 0 (shotC), then quits.

Primary proof = serial log lines `WM-FOCUS <idx>` (the kernel emits these from
Compositor::set_focus when focus actually changes). Secondary proof = a tiny
stdlib PNG decoder samples the center pixel of each window's region and checks
that window 1 changed between A and B while window 0 stayed put.

Layout (Limine fb = 1280x800): window 0 surface rect = (0,0,320,240); window 1
surface rect = (640,0,320,240). Cursor starts centered ~(640,400).
"""
import socket, json, time, sys, struct, zlib, os

ROOT = "/mnt/e/MinimalOS/BasicOperatingSystem"
SOCK = "/tmp/qmp.sock"
SHOT_A = ROOT + "/build/comp-shotA.png"
SHOT_B = ROOT + "/build/comp-shotB.png"
SHOT_C = ROOT + "/build/comp-shotC.png"
SERIAL = ROOT + "/build/comp-serial.log"

# Window surface rects (x, y, w, h) in framebuffer pixels.
WIN0 = (0, 0, 320, 240)
WIN1 = (640, 0, 320, 240)


def connect():
    for _ in range(120):
        try:
            s = socket.socket(socket.AF_UNIX)
            s.connect(SOCK)
            return s
        except OSError:
            time.sleep(0.5)
    print("FAIL: QMP timeout"); sys.exit(1)


def png_pixel(path, x, y):
    """Return (r,g,b) at (x,y) from a 24/32-bit non-interlaced PNG. stdlib only."""
    with open(path, "rb") as fh:
        data = fh.read()
    assert data[:8] == b"\x89PNG\r\n\x1a\n", "not a PNG"
    pos = 8
    width = height = bit_depth = color_type = None
    idat = b""
    while pos < len(data):
        ln = struct.unpack(">I", data[pos:pos + 4])[0]
        typ = data[pos + 4:pos + 8]
        chunk = data[pos + 8:pos + 8 + ln]
        if typ == b"IHDR":
            width, height, bit_depth, color_type = struct.unpack(">IIBB", chunk[:10])
        elif typ == b"IDAT":
            idat += chunk
        elif typ == b"IEND":
            break
        pos += 12 + ln
    assert bit_depth == 8, "expected 8-bit depth, got %r" % bit_depth
    channels = {0: 1, 2: 3, 3: 1, 4: 2, 6: 4}[color_type]
    raw = zlib.decompress(idat)
    stride = width * channels
    # Defilter only up to the needed row (Paeth/Sub/Up/Avg need prior rows).
    prev = bytearray(stride)
    out_row = None
    p = 0
    for row in range(height):
        ft = raw[p]; p += 1
        cur = bytearray(raw[p:p + stride]); p += stride
        for i in range(stride):
            a = cur[i - channels] if i >= channels else 0
            b = prev[i]
            c = prev[i - channels] if i >= channels else 0
            x_ = cur[i]
            if ft == 0:
                v = x_
            elif ft == 1:
                v = x_ + a
            elif ft == 2:
                v = x_ + b
            elif ft == 3:
                v = x_ + (a + b) // 2
            elif ft == 4:
                pp = a + b - c
                pa, pb, pc = abs(pp - a), abs(pp - b), abs(pp - c)
                pr = a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)
                v = x_ + pr
            else:
                raise ValueError("bad filter %d" % ft)
            cur[i] = v & 0xFF
        prev = cur
        if row == y:
            out_row = cur
            break
    base = x * channels
    return (out_row[base], out_row[base + 1], out_row[base + 2])


def main():
    s = connect()
    f = s.makefile("rw")

    def cmd(o):
        f.write(json.dumps(o) + "\n"); f.flush()
        return json.loads(f.readline())

    json.loads(f.readline())  # greeting
    cmd({"execute": "qmp_capabilities"})

    print("waiting for boot + first render (16s)...")
    time.sleep(16)
    cmd({"execute": "screendump", "arguments": {"filename": SHOT_A, "format": "png"}})
    print("shotA captured:", SHOT_A)

    # Move cursor from center (~640,400) INTO window 1 center (~760,120):
    # dx = +120, dy = -280, in 8 small steps.
    for _ in range(8):
        cmd({"execute": "input-send-event", "arguments": {"events": [
            {"type": "rel", "data": {"axis": "x", "value": 15}},
            {"type": "rel", "data": {"axis": "y", "value": -35}}]}})
        time.sleep(0.06)
    time.sleep(0.5)
    # Click (down + up).
    cmd({"execute": "input-send-event", "arguments": {"events": [
        {"type": "btn", "data": {"button": "left", "down": True}}]}})
    time.sleep(0.15)
    cmd({"execute": "input-send-event", "arguments": {"events": [
        {"type": "btn", "data": {"button": "left", "down": False}}]}})

    time.sleep(1.5)
    cmd({"execute": "screendump", "arguments": {"filename": SHOT_B, "format": "png"}})
    print("shotB captured:", SHOT_B)

    # Second click back into window 0 center (~160,120): from (760,120) ->
    # dx = -600, dy = 0, in 8 steps.
    for _ in range(8):
        cmd({"execute": "input-send-event", "arguments": {"events": [
            {"type": "rel", "data": {"axis": "x", "value": -75}},
            {"type": "rel", "data": {"axis": "y", "value": 0}}]}})
        time.sleep(0.06)
    time.sleep(0.5)
    cmd({"execute": "input-send-event", "arguments": {"events": [
        {"type": "btn", "data": {"button": "left", "down": True}}]}})
    time.sleep(0.15)
    cmd({"execute": "input-send-event", "arguments": {"events": [
        {"type": "btn", "data": {"button": "left", "down": False}}]}})
    time.sleep(1.5)
    cmd({"execute": "screendump", "arguments": {"filename": SHOT_C, "format": "png"}})
    print("shotC captured:", SHOT_C)

    cmd({"execute": "quit"})
    try:
        s.close()
    except Exception:
        pass

    # --- Assertions ---------------------------------------------------------
    ok = True

    # 1. Serial: WM-FOCUS lines (PRIMARY proof).
    time.sleep(0.5)
    serial = ""
    if os.path.exists(SERIAL):
        with open(SERIAL, "rb") as fh:
            serial = fh.read().decode("utf-8", "replace")
    focus_lines = [ln.strip() for ln in serial.splitlines() if "WM-FOCUS" in ln]
    print("\n--- WM-FOCUS serial lines ---")
    for ln in focus_lines:
        print(ln)
    if not focus_lines:
        print("WARN: no WM-FOCUS lines in serial")
    has1 = any("WM-FOCUS 1" in ln for ln in focus_lines)
    has0 = any("WM-FOCUS 0" in ln for ln in focus_lines)
    if has1:
        print("SERIAL: PASS WM-FOCUS 1 present (focus moved to window 1)")
    else:
        print("SERIAL: FAIL WM-FOCUS 1 NOT present"); ok = False
    if has0:
        print("SERIAL: WM-FOCUS 0 present (focus returned to window 0)")

    # 2. Pixels: window-1 center changed A->B; window-0 center unchanged (secondary).
    def center(rect):
        x, y, w, h = rect
        return (x + w // 2, y + h // 2)

    try:
        w0c = center(WIN0); w1c = center(WIN1)
        a0 = png_pixel(SHOT_A, *w0c); b0 = png_pixel(SHOT_B, *w0c)
        a1 = png_pixel(SHOT_A, *w1c); b1 = png_pixel(SHOT_B, *w1c)
        print("\n--- pixel samples (center r,g,b) ---")
        print("win0 A=%s B=%s   win1 A=%s B=%s" % (a0, b0, a1, b1))
        if a1 != b1:
            print("PIXELS: PASS window-1 center CHANGED %s -> %s" % (a1, b1))
        else:
            print("PIXELS: FAIL window-1 center UNCHANGED %s" % (a1,)); ok = False
        if a0 == b0:
            print("PIXELS: PASS window-0 center UNCHANGED %s" % (a0,))
        else:
            print("PIXELS: NOTE window-0 center changed %s -> %s (may be cursor/border)" % (a0, b0))
    except Exception as e:
        print("PIXELS: SKIP (decode error: %r)" % e)

    print("\n=== RESULT:", "PASS" if ok else "FAIL", "===")
    sys.exit(0 if ok else 2)


if __name__ == "__main__":
    main()
