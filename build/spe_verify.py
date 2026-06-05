#!/usr/bin/env python3
"""SP-E compositor visual verifier: the four DESKTOP APPS as compositor windows.

SP-D proved the userspace shell boots as the full-screen background and its
"☰ Apps" launcher spawns ONE app (egui-demo). SP-E ports the four gui-core
DeskApps (About / Files / Terminal / Activity Monitor) into thin `ruos-window`
crates shipped to `/bin/{about,files,terminal,system}.cwasm`, so the OTHER four
launcher rows now spawn real app windows.

Boots build/comptest.iso headless under QEMU+QMP. Each egui wasm instance reserves
~48 MiB (0x3000000) of guest linear memory; the bg shell + one app fits easily in
the 256 MiB heap, but the shell + several apps does NOT. To get a clean window
shot per app WITHOUT fighting close-button geometry, the DEFAULT mode boots a
FRESH QEMU per app: open the "☰ Apps" menu (click ~ (30,12)), click the app's row,
wait for the egui window, screendump, quit. Sequence (one boot each):

  spe-0-desktop   the DESKTOP: wallpaper + top panel ("☰ Apps" left, clock + ⏻).
  spe-1-about     About row → a CSD window titled "About ruOS" + content.
  spe-2-files     Files row → a CSD window titled "Files".
  spe-3-terminal  Terminal row → a CSD window titled "Terminal" (a text field).
  spe-4-system    System Monitor row → "Activity Monitor" (proc table + CPU charts).
  spe-5-multi     (--multi) About + Files open together (multi-window shot).

`--single` runs the original one-boot sequence (opens all four, closing each [X]
before the next — depends on close-button geometry, hits the heap budget if a
close misses). DEFAULT (no flag) = per-app fresh boots (robust) + a 2-app multi
boot for spe-5-multi.

Primary proof = `wm.spawn ok name='{about,files,terminal,system}'` in the serial
(one per app the launcher spawned) + the launched windows in spe-1..4.

Launcher catalog order (ruos-desktop/shell CATALOG): egui-demo / About / Files /
Terminal / System Monitor. The dropped menu rows (Limine fb = 1280x800):
  egui-demo y≈37, About ≈58, Files ≈79, Terminal ≈100, System ≈121.
The "☰ Apps" button is top-left ~ (30,12).
"""
import socket, json, time, sys, os, subprocess, signal

ROOT = "/mnt/e/MinimalOS/BasicOperatingSystem"
SOCK = "/tmp/qmp.sock"
ISO = ROOT + "/build/comptest.iso"
SERIAL = ROOT + "/build/spe-serial.log"

SHOT_DESKTOP = ROOT + "/build/spe-0-desktop.png"
SHOT_MULTI = ROOT + "/build/spe-5-multi.png"

FB_W, FB_H = 1280, 800
START_X, START_Y = 640, 400

# Top panel "☰ Apps" menu button: very top-left of the screen.
APPS_BTN = (30, 12)

# The four apps to spawn, in launcher order: name, menu-row (x,y), screendump.
# Row Y from the SP-D menu screendump; X ~40 (inside the dropped menu column).
APPS = [
    ("about",    (40, 58),  ROOT + "/build/spe-1-about.png"),
    ("files",    (40, 79),  ROOT + "/build/spe-2-files.png"),
    ("terminal", (40, 100), ROOT + "/build/spe-3-terminal.png"),
    ("system",   (40, 121), ROOT + "/build/spe-4-system.png"),
]

# Cascade placement (kernel wm.rs spawn_named): the first app over the bg shell
# (live=1) is placed at origin (68,68), then grows to its committed size. The CSD
# [X] is at the far-right of the ~24px titlebar (right-to-left, ~4px margin).
# About/Files/Terminal commit 560x420 → right edge x≈628 → [X] ≈ (614,80).
# System commits 720x520 → right edge x≈788 → [X] ≈ (774,80). Used only by
# --single mode (closing one app before opening the next).
CLOSE_XY = {
    "about":    (614, 80),
    "files":    (614, 80),
    "terminal": (614, 80),
    "system":   (774, 80),
}


def launch_qemu():
    if os.path.exists(SOCK):
        os.remove(SOCK)
    if os.path.exists(SERIAL):
        os.remove(SERIAL)
    cmd = [
        "qemu-system-x86_64",
        "-machine", "q35,accel=kvm:tcg",
        "-cpu", "max",
        "-m", "512",
        "-no-reboot",
        "-display", "none",
        "-serial", "file:" + SERIAL,
        "-qmp", "unix:%s,server,nowait" % SOCK,
        "-device", "qemu-xhci",
        "-cdrom", ISO,
    ]
    print("launching QEMU:", " ".join(cmd))
    return subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def connect():
    for _ in range(120):
        try:
            s = socket.socket(socket.AF_UNIX)
            s.connect(SOCK)
            return s
        except OSError:
            time.sleep(0.5)
    print("FAIL: QMP timeout"); sys.exit(1)


class Session:
    """One booted QEMU + its QMP connection, with mouse/screendump helpers."""

    def __init__(self):
        self.qemu = launch_qemu()
        self.s = connect()
        self.f = self.s.makefile("rw")
        json.loads(self.f.readline())  # greeting
        self.cmd({"execute": "qmp_capabilities"})
        self.cur = [START_X, START_Y]

    def cmd(self, o):
        self.f.write(json.dumps(o) + "\n"); self.f.flush()
        return json.loads(self.f.readline())

    def move_to(self, tx, ty):
        dx = tx - self.cur[0]
        dy = ty - self.cur[1]
        steps = 16
        for k in range(steps):
            sx = (dx - dx * k // steps) - (dx - dx * (k + 1) // steps)
            sy = (dy - dy * k // steps) - (dy - dy * (k + 1) // steps)
            self.cmd({"execute": "input-send-event", "arguments": {"events": [
                {"type": "rel", "data": {"axis": "x", "value": sx}},
                {"type": "rel", "data": {"axis": "y", "value": sy}}]}})
            time.sleep(0.04)
        self.cur[0], self.cur[1] = tx, ty
        time.sleep(0.3)

    def wiggle(self):
        # A tiny move so the kernel forwards a fresh window-local move positioning
        # egui's pointer precisely before the press.
        self.cmd({"execute": "input-send-event", "arguments": {"events": [
            {"type": "rel", "data": {"axis": "x", "value": 1}}]}})
        time.sleep(0.1)
        self.cmd({"execute": "input-send-event", "arguments": {"events": [
            {"type": "rel", "data": {"axis": "x", "value": -1}}]}})
        time.sleep(0.1)

    def btn(self, down):
        self.cmd({"execute": "input-send-event", "arguments": {"events": [
            {"type": "btn", "data": {"button": "left", "down": down}}]}})

    def click(self):
        # egui needs press + release in distinct compositor frames at the same
        # widget. Hold long enough to span several frames, then release.
        self.btn(True); time.sleep(0.4); self.btn(False); time.sleep(0.2)

    def click_at(self, x, y):
        self.move_to(x, y)
        self.wiggle()
        self.click()

    def shot(self, path):
        self.cmd({"execute": "screendump",
                  "arguments": {"filename": path, "format": "png"}})
        print("captured:", path)

    def quit(self):
        try:
            self.cmd({"execute": "quit"})
        except Exception:
            pass
        try:
            self.s.close()
        except Exception:
            pass
        try:
            self.qemu.send_signal(signal.SIGTERM)
            self.qemu.wait(timeout=5)
        except Exception:
            try:
                self.qemu.kill()
            except Exception:
                pass


def serial_text():
    if not os.path.exists(SERIAL):
        return ""
    with open(SERIAL, "rb") as fh:
        return fh.read().decode("utf-8", "replace")


def spawn_count(name):
    return serial_text().count("wm.spawn ok name='%s'" % name)


def open_app(sess, name, row):
    """Open the ☰ Apps menu and click the app's row; retry up to 4x until the
    serial shows a fresh `wm.spawn ok name='<name>'`. Returns True on spawn."""
    base = spawn_count(name)
    for attempt in range(4):
        sess.click_at(*APPS_BTN)
        time.sleep(0.4)
        sess.click_at(*row)
        time.sleep(2.5)  # VFS load + egui first frame
        if spawn_count(name) > base:
            print("  %s spawned on attempt %d" % (name, attempt + 1))
            return True
        print("  %s: no spawn on attempt %d, retrying ..." % (name, attempt + 1))
    print("  %s: FAILED to spawn after 4 attempts" % name)
    return False


def report(results):
    """Print per-app PASS/FAIL + heap/trap notes from the accumulated serial."""
    serial = serial_text()
    lines = serial.splitlines()
    allocfail = [ln.strip() for ln in lines if "failed to allocate" in ln.lower()]
    notfound = [ln.strip() for ln in lines
                if "wm.spawn:" in ln and "not found" in ln]
    traps = [ln.strip() for ln in lines if " trap" in ln.lower()]

    print("\n--- per-app result (wm.spawn ok) ---")
    ok = True
    for name in ("about", "files", "terminal", "system"):
        if results.get(name):
            print("  %-9s PASS" % name)
        else:
            print("  %-9s FAIL (no wm.spawn ok line)" % name)
            ok = False
    if allocfail:
        print("\nNOTE heap allocation-failure lines:")
        for ln in allocfail[:6]:
            print("  ", ln)
    if notfound:
        print("\nNOTE wm.spawn not-found lines:")
        for ln in notfound[:6]:
            print("  ", ln)
    if traps:
        print("\nNOTE trap lines:")
        for ln in traps[:6]:
            print("  ", ln)
    print("\n=== RESULT:", "PASS" if ok else "PARTIAL/FAIL", "===")
    return ok


def run_per_app():
    """DEFAULT: a fresh QEMU per app (bg shell + one app = clean heap). Captures
    spe-0-desktop on the first boot, then spe-1..4. Then a 2-app --multi boot."""
    results = {}
    # Accumulate serial across boots so the final report sees every spawn line.
    accumulated = ""
    for i, (name, row, shotpath) in enumerate(APPS):
        print("=== fresh boot for app '%s' (row %r) ===" % (name, row))
        sess = Session()
        print("  waiting for boot + desktop render (18s)...")
        time.sleep(18)
        if i == 0:
            sess.shot(SHOT_DESKTOP)  # the bare desktop, captured once
        ok = open_app(sess, name, row)
        results[name] = ok
        time.sleep(0.5)
        sess.shot(shotpath)
        sess.quit()
        accumulated += serial_text() + "\n"
        time.sleep(0.5)

    # Multi-window shot: About + Files together in one boot (2 apps + shell fits).
    print("=== multi-window boot: About + Files ===")
    sess = Session()
    print("  waiting for boot + desktop render (18s)...")
    time.sleep(18)
    open_app(sess, "about", APPS[0][1])
    time.sleep(0.5)
    open_app(sess, "files", APPS[1][1])
    time.sleep(0.5)
    sess.shot(SHOT_MULTI)
    sess.quit()
    accumulated += serial_text() + "\n"

    # Write the accumulated serial back so report()/grep see every spawn line.
    with open(SERIAL, "w") as fh:
        fh.write(accumulated)
    return results


def run_single():
    """--single: one boot, open all four sequentially, closing each [X] before the
    next (depends on close-button geometry; may hit the heap budget)."""
    sess = Session()
    print("waiting for boot + desktop render (18s)...")
    time.sleep(18)
    sess.shot(SHOT_DESKTOP)
    results = {}
    for name, row, shotpath in APPS:
        print("=== opening app '%s' (row %r) ===" % (name, row))
        ok = open_app(sess, name, row)
        results[name] = ok
        time.sleep(0.5)
        sess.shot(shotpath)
        print("  closing '%s' window via [X] %r ..." % (name, CLOSE_XY[name]))
        sess.click_at(*CLOSE_XY[name])
        time.sleep(0.8)
    sess.quit()
    return results


def main():
    if "--single" in sys.argv:
        results = run_single()
    else:
        results = run_per_app()
    ok = report(results)
    sys.exit(0 if ok else 2)


if __name__ == "__main__":
    main()
