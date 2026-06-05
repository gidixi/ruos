#!/usr/bin/env python3
"""Minimal QMP screendump driver for the SP4 compositor equivalence test.

Connects to a running QEMU's QMP unix socket, sends `qmp_capabilities`, waits a
fixed number of seconds for the compositor to render its steady two-window
composite, takes a single `screendump` (PNG) to the requested path, then `quit`s
the VM. NO input injection — the reactor apps are static at boot, so a screendump
after warm-up is deterministic and independent of frame timing (see SP4 facts).

Usage:
    comp_shot.py <qmp.sock> <out.png> <wait_seconds>

Modelled on build/comp_verify.py's connect + qmp_capabilities + screendump +
quit pattern (SP2), stripped to the equivalence-test essentials.
"""
import socket, json, time, sys


def connect(sock_path):
    """Retry-connect to the QMP unix socket while QEMU is still coming up."""
    for _ in range(120):
        try:
            s = socket.socket(socket.AF_UNIX)
            s.connect(sock_path)
            return s
        except OSError:
            time.sleep(0.5)
    print("FAIL: QMP timeout connecting to %s" % sock_path)
    sys.exit(1)


def main():
    if len(sys.argv) != 4:
        print("usage: comp_shot.py <qmp.sock> <out.png> <wait_seconds>")
        sys.exit(2)
    sock_path = sys.argv[1]
    out_png = sys.argv[2]
    wait_s = float(sys.argv[3])

    s = connect(sock_path)
    f = s.makefile("rw")

    def cmd(o):
        f.write(json.dumps(o) + "\n"); f.flush()
        return json.loads(f.readline())

    json.loads(f.readline())          # QMP greeting
    cmd({"execute": "qmp_capabilities"})

    print("waiting %.0fs for boot + steady composite..." % wait_s)
    time.sleep(wait_s)

    cmd({"execute": "screendump",
         "arguments": {"filename": out_png, "format": "png"}})
    print("screendump captured:", out_png)

    cmd({"execute": "quit"})
    try:
        s.close()
    except Exception:
        pass


if __name__ == "__main__":
    main()
