# 110 — Userspace coreutils batch (22 .wasm tool)

**Data:** 2026-05-29

## Cosa
22 nuovi crate `wasm32-wasip1` in `user/`:
- File ops: `mkdir`, `rmdir`, `rm` (con -r/-f), `cp` (con -r), `mv`
- Text/search: `head`, `tail`, `grep` (-rn), `find` (-name + glob *),
  `diff` (naive line-by-line), `du` (-sh)
- Sysinfo: `whoami`, `id`, `uname` (-a), `uptime`, `free` (-h),
  `df` (-h, una riga tmpfs), `lscpu`, `dmesg`
- Proc: `ps`, `kill`, `pkill`

Pattern: ciascuno parsing argv via `std::env::args`, I/O via
`std::fs`/`std::io` (mappato a WASI dal wasi-libc). Tool che devono
camminare directory (`rm -r`, `cp -r`, `grep -r`, `find`, `du`)
inlineano un helper `readdir` che chiama la host fn custom
`ruos_readdir` (`std::fs::read_dir` non wired — manca `fd_readdir`
WASI). Tool sysinfo/proc chiamano direttamente le host fns
`ruos_uname/uptime/meminfo/cpuinfo/dmesg/proc_list/proc_kill`.

Workspace `user/Cargo.toml` aggiornato con i 22 nuovi member.
Makefile usa un singolo pattern rule `user-bin/%.wasm` invece di una
ricetta esplicita per ciascun tool (la lista è ora in `BIN_TOOLS`).
`limine.conf` aggiunge i 22 `module_path`/`module_cmdline` necessari
così il kernel li monta in `/bin/` al boot.

`/etc/init.sh` esteso con uno smoke test (mkdir/cp/mv/cat/head/tail/
du/grep/find/rm/rm -r/free/df/lscpu/ps) per validare end-to-end.

## Perché
Estende il set di comandi disponibili nella shell oltre i 4 originali
(ls, cat, echo + shell builtins). Sblocca uso interattivo realistico.

## File toccati
- user/Cargo.toml
- user/{mkdir,rmdir,rm,cp,mv,head,tail,grep,find,diff,du,whoami,id,
  uname,uptime,free,df,lscpu,dmesg,ps,kill,pkill}/{Cargo.toml,src/main.rs}
  (44 file nuovi)
- Makefile (pattern rule + lista BIN_TOOLS)
- limine.conf
- user-bin/init.sh
