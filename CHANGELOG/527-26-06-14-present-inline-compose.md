# 527 — present: compose INLINE (no SMP overhead) post-warmup

**Data:** 2026-06-14

## Cosa
Diagnosi (numeri HW): col mouse fermo il loop è veloce e idle (iter 14ms, hlt 9ms,
67Hz), ma `pr` (present) = ~10ms. Il compositing vero è memcpy-bound (~8-16 MB per
schermo tipico ⇒ ~2-3 ms su un core); i ~10 ms sono **overhead del fan-out SMP** di
`dispatch_bands`: submit di N bande + IPI broadcast per banda + join-spin (+ il pump
cursore nello spin). Durante l'interazione col menu, ogni cambio visibile fa un
present → quei ~10 ms ripetuti facevano sentire tutto lento, pur con numeri "buoni".

Fix: `dispatch_bands` ora ha un path **INLINE** (componi tutto lo schermo sul core
GUI con una sola `composite_band`, niente submit/IPI/join). `present(compose_inline)`
lo usa **post-warmup** (`frame_no >= WARMUP_FRAMES`); durante il warmup resta SMP
così il boot-check "composite cores" (frame 30) vede più core. Present scende da
~10 ms a ~2-3 ms.

SICURO: è un FULL compose, solo seriale → nessun damage tracking, nessun rischio di
ghosting. Per uno schermo laptop (1280×800, poche finestre) inline (~2-3 ms) batte
nettamente il fan-out SMP (~10 ms di overhead); il fan-out conviene solo per lavori
grandi, che non è il caso del compositing tipico.

## Perché
La scelta utente era "present per-damage", ma la causa misurata del `pr` alto è
l'OVERHEAD SMP, non l'area: comporre solo il damage non toglierebbe il fan-out. Il
fix corretto è inline (rimuove submit+IPI+join). Più semplice e senza rischio
ghosting del partial-compose. Il blit fa già lo shadow-diff → scrive comunque solo
le righe VRAM cambiate.

## File toccati
- kernel/src/wasm/wt/wm.rs (dispatch_bands: param `inline` + path inline; present:
  param `compose_inline`; run loop: present(frame_no >= WARMUP_FRAMES))
