# Compositor GATE: launch two reactor windows side-by-side and keep them up
# (run_compositor_gate owns the CPU, never returns) so the steady two-window
# composite is visible for inspection. `compositor` resolves to
# /bin/compositor.cwasm (the reactor cwasm shipped under that name), same as
# `gui` resolves to /bin/gui.cwasm.
echo ruos boot OK
compositor
