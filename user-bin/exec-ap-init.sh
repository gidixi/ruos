# C2b gate: exec a .cwasm from the shell so exec_worker routes it to a ComputeApp
# core. wtecho resolves to /bin/wtecho.cwasm (staged by the Makefile).
# The serial log will show: exec-ap ran_on=core<N> (N>=1 means off the BSP).
# The shell receives wtecho's stdout via the PTY, which reaches serial as EXEC_AP_OK.
wtecho EXEC_AP_OK
echo ruos boot OK
