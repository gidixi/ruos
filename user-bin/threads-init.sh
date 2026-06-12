# MT Fase 2 threads init — eseguito da tests/threads-test.sh (stage 2).
# parsum: rayon end-to-end (PARSUM_OK threads=N); mtstress: Mutex conteso
# + join (STRESS_MT_OK count=400000); mtstress trap: un thread abortisce ->
# kill-group exit 134, la shell DEVE sopravvivere e stampare il marker finale.
parsum
mtstress
mtstress trap
echo THREADS_INIT_DONE
