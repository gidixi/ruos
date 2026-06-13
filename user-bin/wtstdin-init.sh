# Deterministic smoke for wt-readline: launched on the boot PTY (no live
# input → stdin EOF on the BSP) it must still print READY and exit cleanly,
# proving the .cwasm instantiates and the fd_read fd0 path returns EOF.
wtreadline
echo WTSTDIN_INIT_DONE
