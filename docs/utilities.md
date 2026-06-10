# Utility userland ruos — riferimento

Documento generato leggendo i sorgenti di **tutte** le utility WASM in `user/*/src/main.rs`.
Per ogni tool: cosa fa, comportamento effettivamente implementato (flag/argomenti
onorati, internals), e cosa manca rispetto alla versione reale (GNU coreutils /
util-linux / standard POSIX).

**Ultimo aggiornamento:** 2026-06-10 — ~59 programmi (`build/iso_root/bin/*.wasm`).

---

## Architettura comune

- **Target:** `wasm32-wasip1` con `std` (WASI Preview 1). I/O su file via `std::fs`
  (`path_open`/`fd_read`/`fd_write`/`path_filestat_get`), stdin/stdout via `std::io`.
- **Host functions ruos** (`#[link(wasm_import_module = "ruos")]`): syscall custom del
  kernel, non WASI. Pattern tipico `fn(buf_ptr, buf_len, used_ptr) -> errno`: il kernel
  scrive un blob e ritorna i byte usati via `used_ptr`. `errno 0` = ok, `errno 8` =
  ENOBUFS (riprovare con buffer più grande).
- **`readdir`**: `std::fs::read_dir` **non è cablato**. L'enumerazione directory passa
  per la host fn `ruos::readdir`. Formato record: header 12 byte
  `[0]=kind (0=REG,1=DIR,2=DEV)`, `[2..4]=name_len u16 LE`, `[4..12]=size u64 LE`,
  seguito da `name_len` byte UTF-8 del nome.
- **Buffer cap fissi**: ls=8192, cp/rm/du/find/grep=16384, ps/pkill=8192, dmesg=32768.
  Directory/log oltre il cap vengono **troncati silenziosamente** (solo `lspci`/`lsusb`/
  `ip`/`ifconfig` riprovano su ENOBUFS).
- **Decodifica UTF-8**: quasi sempre `from_utf8_lossy` (sostituisce byte invalidi).
- **Input bufferizzato**: la maggior parte legge tutto in `Vec<u8>` (no streaming).
- **Parsing flag**: ad-hoc, posizionale, match per uguaglianza esatta. Nessun tool
  supporta opzioni long-form `--xxx` (salvo poche eccezioni elencate). Diversi tool non
  rilevano i `-` (es. `rmdir`, `touch`) e tratterebbero un flag come nome file.
- **Nessun modello di permessi/timestamp/utenti**: spiega `touch` che non tocca i tempi,
  `cp` che non preserva i mode, `id`/`whoami` hardcoded.

---

## Indice

| Tool | Categoria | Funzione |
|------|-----------|----------|
| shell | sistema | shell interattiva con history/completion/pipe/exec wasm |
| nano | editor | editor full-screen 80×24 |
| ls cat cp mv rm mkdir rmdir touch | file | gestione file/directory |
| du df diff head tail | file | dimensioni / diff / porzioni |
| wc sort uniq cut tr tee echo clear which | testo | manipolazione testo |
| ps kill pkill | processi | gestione processi |
| rtop | processi | htop-style monitor full-screen (ratatui) |
| service | sistema | gestione servizi kernel |
| uname uptime free lscpu lspci lsusb dmesg id whoami date | sistema | info sistema |
| ip ifconfig nc ping wget | rete | networking |
| wifiscan wificonnect | rete | Wi-Fi scan/connect (RTL8188EU) |
| find grep | ricerca | ricerca file/contenuti |
| disks umount | disco | elenco dischi SATA / unmount |
| mkdisk mkboot install | disco | disk authoring / installer SSD |
| gzip gunzip zcat | compressione | gzip RFC 1952 compress/decompress |
| init client server | speciali | init + test stub socket |
| base64 | utility | codifica/decodifica dati |
| sha256sum | crittografia | hash file |

---

## Shell ed editor

### shell
- **Funzione:** shell interattiva con line editing, history, tab completion, pipe,
  builtin, exec di `.wasm`.
- **Implementato:** nessun flag CLI. Builtin: `cd`, `pwd`, `exit`, `help`, `poweroff`,
  `reboot`, `source`/`.`. Comandi esterni risolti come `/bin/<cmd>.wasm` (o path letterale
  se contiene `/`; su SSD anche `/mnt/bin/<cmd>.wasm`). Host fn ruos: `exec`,
  `exec_pipeline`, `readdir`, `tcgetattr`/`tcsetattr`, `chdir`, `poweroff`, `reboot`.
  Raw mode (clear `ICANON|ECHO|ISIG`). Editor: backspace, Ctrl-A (home), Ctrl-E (fine),
  Ctrl-L (clear), Ctrl-C (annulla riga), Tab (completion builtin + `/bin/*.wasm` e path
  filesystem), frecce Su/Giù (history), Sx/Dx (cursore). **Pipe `|`**: le righe con `|`
  sono splittate in segmenti, serializzate in un blob binario e passate alla host fn
  `exec_pipeline` che le esegue come fiber concorrenti collegate da pipe in-RAM (max 4
  stadi). Builtin non ammessi in pipeline. Avvio: esegue `/etc/init.sh` riga per riga,
  banner verde. `exec` passa argv come blob binario (count u32 + tabella offset/len +
  byte). HISTORY globale in `Mutex<Vec<String>>`.
- **Manca dal reale (bash/sh):** niente redirezioni (`>` `<` `>>`  `2>`), job
  control (`&` `fg` `bg` Ctrl-Z). Niente variabili/`export`/espansione env, command
  substitution `$()`/backtick, globbing (`*`/`?` passati letterali), quoting (split su
  whitespace puro), control flow (`if`/`for`/`while`/`case`/funzioni), sequencing
  (`;` `&&` `||`), `~`, `$PATH` (hardcoded `/bin`), alias. History non persistita, no
  Ctrl-R, no builtin `history`. `exit` ignora il codice (sempre 0). Pipeline max 4
  stadi, non infinita.

### nano
- **Funzione:** editor di testo full-screen 80×24 (apri, modifica, salva).
- **Implementato:** un solo argomento posizionale `<file>` (obbligatorio, altrimenti
  exit 2). Host fn `tcgetattr`/`tcsetattr` per raw mode. Buffer `Vec<String>` (una riga
  per elemento). Geometria fissa `COLS=80 ROWS=24 VIEWPORT=22` (2 righe footer: status bar
  invertita + help). Render full-screen ad ogni frame (`\x1b[2J\x1b[H`), righe troncate a
  80 char, `~` oltre EOF. Tasti: ASCII stampabili (insert), Backspace (cancella/unisce),
  Enter (`split_off` → nuova riga), frecce con wrap, Home/End, `^O` salva, `^X` esce.
  Salvataggio: join con `\n` + newline finale, `fs::write`.
- **Manca dal reale (GNU nano):** niente prompt salva-all'uscita (`^X` scarta le modifiche),
  niente ricerca/replace (`^W`/`^\`), cut/paste (`^K`/`^U`), goto-line (`^_`), undo/redo,
  help (`^G`). Nessun flag (`-w` `-m` `-i` `-c` …). Niente scroll orizzontale (righe >80
  troncate in vista), no wrap, no tab-width. Input non UTF-8 multibyte (un byte = un char).
  No syntax highlighting, no `.nanorc`, dimensioni fisse 80×24 (ignora il terminale reale),
  no read-only, no backup.

---

## File e directory

### ls
- **Funzione:** elenca le entry di una singola directory con tipo, size, nome.
- **Implementato:** un path posizionale opzionale (default `.`). Nessun flag. Host
  `readdir` (buf 8192). Stampa `KIND size name[mark]` con KIND ∈ {REG,DIR,DEV,???},
  mark `/` per dir, `@` per dev. Nomi non-UTF-8 → `?`.
- **Manca dal reale:** nessun flag (`-l` `-a`/`-A` `-h` `-R` `-t`/`-S`/`-r` `-1` `-d`
  `-i`, colori, layout multicolonna). Output in ordine raw di readdir (non ordinato).
  Operando singolo. Buffer 8192 → directory grandi troncate.

### cat
- **Funzione:** stampa il contenuto di un file su stdout.
- **Implementato:** esattamente un path posizionale. Nessun flag. `File::open` +
  `read_to_end`; se UTF-8 valido stampa, altrimenti stampa il placeholder
  `(binary, N bytes)`.
- **Manca dal reale:** niente stdin (`-` o no-arg), niente concatenazione di più file,
  niente `-n`/`-b` (numera), `-E`/`-A`/`-v`, `-s` (squeeze). **Non** passa i dati binari
  byte-per-byte (li sostituisce con una stringa di riepilogo).

### cp
- **Funzione:** copia un file, o ricorsivamente un albero di directory.
- **Implementato:** `-r`/`-R`/`--recursive`. Due operandi (src, dst) obbligatori. Copia
  file via buffer 4096; copia ricorsiva via `metadata` + `create_dir` + `readdir` (16384).
- **Manca dal reale:** dst dev'essere path finale esplicito — niente "copia DENTRO una
  directory" (no append basename), niente sorgenti multiple. Niente `-p`/`--preserve`
  (mode/timestamp), `-i`/`-n`/`-f` (sovrascrive sempre), `-u`, `-l`/`-s` (link), `-a`,
  `-v`, `--parents`. Niente symlink. Buffer 16384 → dir grandi troncate.

### mv
- **Funzione:** rinomina/sposta un path in un altro.
- **Implementato:** due operandi posizionali. `std::fs::rename(src, dst)`.
- **Manca dal reale:** nessun flag (`-i`/`-n`/`-f` `-u` `-v` `-t`/`-T`). Niente sorgenti
  multiple in directory, niente semantica "sposta dentro directory", **niente fallback
  cross-filesystem** (copy+delete): un rename tra mount/device fallisce.

### rm
- **Funzione:** rimuove file, o ricorsivamente alberi di directory.
- **Implementato:** `-r`/`-R`/`--recursive`, `-f`/`--force`, combinati `-rf`/`-fr`. Path
  multipli. Ricorsione via `metadata` + `readdir` (16384) + `remove_dir`. Con `force` gli
  errori sono soppressi.
- **Manca dal reale:** riconosce solo i bundle esatti `-rf`/`-fr`. Niente `-i`/`-I`
  (prompt), `-d` (dir vuota senza `-r`), `-v`, `--preserve-root` (**nessuna protezione
  root**), `--one-file-system`. `force` non sopprime l'errore "operando mancante".

### mkdir
- **Funzione:** crea una o più directory.
- **Implementato:** `-p`/`--parents`. Path multipli. `create_dir_all` se `parents` else
  `create_dir`.
- **Manca dal reale:** niente `-m`/`--mode` (nessun modello permessi), `-v`/`--verbose`,
  `-Z`/context.

### rmdir
- **Funzione:** rimuove directory vuote.
- **Implementato:** solo path posizionali multipli (**nessun parsing flag**).
  `std::fs::remove_dir` per path.
- **Manca dal reale:** niente `-p`/`--parents`, `--ignore-fail-on-non-empty`, `-v`.
  Qualsiasi `-flag` verrebbe trattato come nome (passato a `remove_dir`).

### touch
- **Funzione:** crea file se non esistono (o apre esistenti).
- **Implementato:** operandi multipli (nessun flag). `OpenOptions.create(true).write(true)`.
- **Manca dal reale:** **non aggiorna i tempi** (scopo principale di touch reale): assicura
  solo l'esistenza. Niente `-c` `-a`/`-m` `-r`/`--reference` `-t`/`-d`/`--date` `-h`.
  Nessun parsing `-` (un flag è trattato come filename). Nessun modello di timestamp nell'OS.

### du
- **Funzione:** riporta la dimensione totale in byte dei file sotto i path dati.
- **Implementato:** `-s`, `-h`, combinati `-sh`/`-hs`. Path multipli (default `.`).
  Ricorsione via `metadata` + `readdir` (16384) sommando dimensioni file. Le directory
  contano 0. **`-s` è parsato ma inerte** (`let _ = summary;`): è sempre di fatto summary.
- **Manca dal reale:** modo non-summary assente (du reale stampa ogni sottodirectory).
  Riporta byte sommati, non blocchi disco — niente `--block-size`/`-B`/`-k`/`-m`,
  `--apparent-size`, `-a` (per-file), `-c` (totale), `-d`/`--max-depth`, `-x`, `--exclude`,
  dedup hardlink. Buffer 16384.

### df
- **Funzione:** mostra l'uso dell'unico filesystem tmpfs montato su `/`.
- **Implementato:** solo `-h`. Host `meminfo` (buf 32): legge `f_total`/`f_used` (frame
  count ×4096). Stampa header + una riga `tmpfs … /`. Senza `-h` formatta in KiB.
- **Manca dal reale:** riporta solo un singolo tmpfs hardcoded derivato dal budget frame
  fisici — non statistiche per-mount reali. Niente argomenti path/filesystem, `-T`, `-i`
  (inode), `-a`, `--total`, `-B`/`--block-size`, `-P`, `--output`.

### diff
- **Funzione:** mostra le differenze riga-per-riga tra esattamente due file di testo.
- **Implementato:** due operandi (altrimenti exit 2). `read_to_string` + `.lines()`.
  Confronto **posizionale naive** indice 0..max: a ogni riga diversa stampa `@@ line N @@`
  + `< a` / `> b`. Exit 1 se differiscono.
- **Manca dal reale:** **niente algoritmo diff reale** (no LCS/Myers) — un singolo
  insert/delete fa cascata su tutte le righe successive. I marker `@@` sono finti (non
  formato unified GNU). Niente `-u`/`-c`/`-y`, `-r`, `-i`, `-w`/`-b`, `-q`, `-N`, stdin,
  detection binari, operandi directory.

### head
- **Funzione:** stampa le prime N righe di file o stdin.
- **Implementato:** `-n <count>` e forma attaccata `-n<count>`. Default 10. Path multipli;
  nessuno → stdin. `BufReader` + `lines()` fino all'indice n.
- **Manca dal reale:** niente `-c`/`--bytes`, `-q`/`-v`, header automatici multi-file
  (`==> name <==`), conteggio negativo `-n -K`, `--lines` long form. Solo a righe.

### tail
- **Funzione:** stampa le ultime N righe di file o stdin.
- **Implementato:** `-n <count>` / `-n<count>` (default 10). **`-f` riconosciuto ma
  rifiutato** ("not supported (no inotify/poll yet)", exit 2). Path multipli; nessuno →
  stdin. Ring buffer `VecDeque` delle ultime n righe.
- **Manca dal reale:** `-f`/`-F` (follow) è errore fisso. Niente `-c`/`--bytes`, `-n +K`,
  `--retry`, `--pid`, `-q`/`-v`, header multi-file. Nessuna ottimizzazione seek-to-end.

---

## Manipolazione testo

### wc
- **Funzione:** conta righe/parole/byte di ogni file (o stdin).
- **Implementato:** `-l`, `-w`, `-c`; operandi file. Default = tutti e tre. `count()`
  itera i byte: lines = `\n`; words = macchina a stati su whitespace; bytes = `len()`.
  Riga `total` solo se >1 file. Larghezza campi `{:8}`, ordine fisso l,w,c.
- **Manca dal reale:** niente `-m` (char), `-L` (max lunghezza riga), `--files0-from`,
  long options. Definizioni byte-based (non multibyte/locale). Nessun exit≠0 su errore.

### sort
- **Funzione:** ordina le righe (file concatenati o stdin) lessicograficamente.
- **Implementato:** `-r` (reverse), `-u` (dedup adiacenti post-sort). Concatena i file,
  `from_utf8_lossy`, split `\n`, `sort()` (Ord byte/Unicode-scalar).
- **Manca dal reale:** niente `-n` (numerico), `-h`, `-g`, `-k`/chiavi, `-t` (sep), `-f`
  (case), `-c` (check), `-m` (merge), `-o` (output), `-s` (stable), `-R`, `-z`. Sort
  puramente bytewise (non locale). Tutto in memoria.

### uniq
- **Funzione:** collassa le righe duplicate adiacenti da file o stdin.
- **Implementato:** `-c` (prefisso conteggio); un solo operando file. Traccia `prev` + `cnt`,
  flush al cambio. Formato conteggio `{:>7}`.
- **Manca dal reale:** niente `-d` (solo dup), `-u` (solo uniche), `-i` (case), `-f N`/
  `-s N`/`-w N` (skip campi/char/width), `-z`, secondo operando OUTPUT. Solo dedup adiacente.

### cut
- **Funzione:** estrae campi o range di caratteri da ogni riga.
- **Implementato:** `-d <delim>` (un char, default TAB), `-f <lista>` (numeri 1-based
  separati da virgola), `-c <range>` (`a-b` o `n`). `-f` ha precedenza su `-c`. Un operando
  file. Emette i campi nell'ordine elencato, rijoin col delimiter.
- **Manca dal reale:** niente `-b` (byte), `-s` (sopprime righe senza delim — qui le stampa
  intere), `--complement`, `--output-delimiter` (sep output = input), `-z`. La lista `-f`
  **non supporta range** (`1-3`, `2-`); `-c` solo un range contiguo. Delimiter single-byte.

### tr
- **Funzione:** traduce (mappa 1:1) o cancella caratteri da stdin a stdout.
- **Implementato:** `tr SET1 SET2` (traduce), `tr -d SET1` (cancella; `-d` deve essere
  primo arg). Map per posizione letterale; se SET2 più corto, padding con l'ultimo char.
- **Manca dal reale:** niente range (`a-z`), classi (`[:upper:]` …), `[c*n]`, escape (`\n`
  `\t` ottali). Niente `-s` (squeeze), `-c`/`-C` (complemento), `-t` (truncate).

### tee
- **Funzione:** copia stdin su stdout e su ogni file indicato.
- **Implementato:** `-a` (append). Legge **tutto** stdin in buffer (non streaming), scrive
  su stdout e su ogni file (`OpenOptions` create + truncate/append).
- **Manca dal reale:** niente `-i`, `--output-error`, `-p`. Input completamente
  bufferizzato (non usabile in pipe illimitata). Exit code sempre 0 anche su errore.

### echo
- **Funzione:** stampa gli argomenti separati da spazio + newline.
- **Implementato:** nessun flag parsato. `args.join(" ")` + newline.
- **Manca dal reale:** niente `-n` (sopprime newline — stampato letteralmente), `-e`/`-E`
  (escape `\n` `\t` `\xHH` …). Nessun processing di escape.

### clear
- **Funzione:** pulisce lo schermo e riporta il cursore in alto.
- **Implementato:** scrive `\x1b[2J\x1b[H` su stdout. Nessun arg.
- **Manca dal reale:** niente terminfo/`TERM` (hardcoded ANSI/VT100), niente `-x`, niente
  erase scrollback (`\x1b[3J`).

### which
- **Funzione:** localizza eseguibili `.wasm` per nome nelle dir di ricerca.
- **Implementato:** operandi multipli. Prova prefissi **hardcoded** `["/bin/","/usr/bin/"]`
  costruendo `{prefix}{name}.wasm`, esistenza via `fs::metadata`. Primo hit stampato.
- **Manca dal reale:** **ignora `$PATH`** (dir hardcoded), estensione `.wasm` forzata.
  Niente `-a` (tutti i match), `-s`, `--skip-dot`/`--skip-tilde`, awareness alias/builtin.
  Nessun check del bit di esecuzione (solo esistenza).

---

## Processi

### ps
- **Funzione:** lista i processi con PID, tempo trascorso, nome comando.
- **Implementato:** nessun flag. Host `proc_list` (buf 8192) + `uptime` (centisecondi
  @100 Hz). Record binari: count u32, poi per record pid u32, start u64 (cs), name_len u16,
  2 pad, nome. ELAPSED = `(now-start)` in `s.cs`.
- **Manca dal reale:** nessun flag (`-e` `-f` `aux` `-o`). Niente UID/utente, TTY, %CPU,
  %MEM, STAT, PPID, ora di avvio, RSS/VSZ. Solo PID/elapsed/nome. Buffer 8192 (no retry).

### kill
- **Funzione:** termina processi per PID.
- **Implementato:** `<pid> [pid...]`. **I `-` iniziali vengono strippati** (`-9`/`-15`
  ignorati come segnale, si tenta il parse del resto). `ruos::proc_kill(pid)`.
- **Manca dal reale:** **nessuna selezione di segnale reale** — `-9`/`-SIGKILL`/`-l`/`-s`
  sono scartati, una sola semantica di kill. `-TERM` (non numerico) fallirebbe come
  "invalid pid". Niente process-group / PID negativi.

### pkill
- **Funzione:** uccide i processi il cui nome contiene una sottostringa.
- **Implementato:** `<substring>` (solo `args[0]`). Stessa decodifica record di `ps`;
  `.contains(pat)` → `proc_kill`. Exit 1 se nessuno ucciso.
- **Manca dal reale:** solo match substring (non regex). Niente `-f` (full cmdline),
  `-signal`, `-u user`, `-x` (esatto), `-n`/`-o`, `-c`. Non stampa i PID.

---

## Informazioni di sistema

### uname
- **Funzione:** stampa l'identificazione di sistema.
- **Implementato:** `-a` (tutti i campi). Host `uname` (buf 256, campi NUL-separati:
  name/node/release/version/machine). `-a` li stampa tutti, altrimenti solo sysname.
- **Manca dal reale:** niente flag per-campo (`-s -n -r -v -m -p -i -o`); solo all-or-sysname.

### uptime
- **Funzione:** stampa l'uptime di sistema.
- **Implementato:** nessun flag. Host `uptime` (i64 centisecondi). Calcola days/h/m/s/cs.
- **Manca dal reale:** niente ora corrente, **niente load average**, niente conteggio utenti,
  niente `-p`/`-s`. Mostra centisecondi (uptime reale no).

### free
- **Funzione:** mostra memoria: frame fisici e heap kernel.
- **Implementato:** `-h`. Host `meminfo` (buf 32: heap_total/used, frame_total/used,
  frame ×4096). Due righe: "phys frames" e "kernel heap". Se `heap_used==0` stampa `?`.
- **Manca dal reale:** **non è il layout Linux `free`** (no Mem:/Swap:/buffers/cache/
  available). Niente swap, shared/buff/cache. Niente `-b/-k/-m/-g/-t/-w/-s/--si`. Heap-used
  forse non disponibile (path `?`).

### lscpu
- **Funzione:** mostra info CPU.
- **Implementato:** nessun flag. Host `cpuinfo` (vendor/brand/ncpu NUL-separati).
  **`Architecture: x86_64` hardcoded**, poi CPU(s)/Vendor ID/Model name.
- **Manca dal reale:** Architecture hardcoded. Solo 4 campi. Niente socket/core/thread, NUMA,
  cache L1/L2/L3, MHz, flags, family/model/stepping, virtualizzazione, modi `-e`/`-p`/`-J`.

### lspci
- **Funzione:** lista i dispositivi PCI.
- **Implementato:** nessun flag. Host `pci_list` (buf 4096, **retry su ENOBUFS** con resize).
  Il kernel ritorna un blob di testo già formattato, stampato verbatim.
- **Manca dal reale:** interamente formattato dal kernel — niente `-v`/`-vv`/`-vvv`,
  `-n`/`-nn`, `-k` (driver), `-s`/`-d` (filtri), `-t` (tree), `-x` (hex).

### lsusb
- **Funzione:** lista i dispositivi USB enumerati dallo stack xHCI. Una riga per
  slot: `Bus BB Dev SS  Port P  Tier T  ID vvvv:pppp  <speed>  <kind>`
  (`Bus`=controller xHCI, `Dev`=slot id, `speed`=Full/Low/High/Super,
  `kind`=Hub/Keyboard/Mouse/Msc/Other).
- **Implementato:** nessun flag. Host `usb_list` (buf 4096, **retry su ENOBUFS**
  con resize, come `lspci`). Il kernel ritorna testo già formattato da
  `registry::usb_list()`, stampato verbatim.
- **Manca dal reale:** niente `-v` (descriptor dump), `-t` (tree), `-s`/`-d`
  (filtri), nome stringa del device (solo VID:PID numerici).

### dmesg
- **Funzione:** stampa il buffer di log del kernel.
- **Implementato:** nessun flag. Host `dmesg` (buf 32 KiB, **niente retry** → log >32 KiB
  troncato). Stampa UTF-8 (lossy fallback).
- **Manca dal reale:** nessun flag — niente `-c` (clear), `-w`/`--follow`, `-l` (level),
  `-f` (facility), `-T`/`--human` (timestamp), `--color`, `-x`. Cap fisso 32 KiB.

### id
- **Funzione:** stampa l'identità utente/gruppo.
- **Implementato:** nessun flag. Stampa la **costante hardcoded**
  `uid=0(root) gid=0(root) groups=0(root)`. Nessuna host call.
- **Manca dal reale:** completamente finto (nessun sottosistema utenti). Niente
  `-u`/`-g`/`-G`/`-n`/`-r`/`-Z`, argomento username, gruppi supplementari.

### whoami
- **Funzione:** stampa l'utente corrente.
- **Implementato:** stampa **hardcoded `root`**. Nessuna host call.
- **Manca dal reale:** costante finta; nessun concetto di utente loggato reale.

### date
- **Funzione:** legge l'RTC e stampa l'ora corrente.
- **Implementato:** `+%s` (epoch unix). `-u` documentato come no-op (RTC assunto UTC).
  Host `time_get` (campi RTC y/m/d/hh/mm/ss + epoch u64). Default `YYYY-MM-DD HH:MM:SS UTC`.
- **Manca dal reale:** solo il formato `+%s` (niente `+FORMAT` strftime generale). Niente
  `-d`/`--date` (parse/set), `-s` (set), `-R`/`--rfc`, gestione timezone (sempre "UTC").

---

## Rete

### ip
- **Funzione:** mostra le interfacce di rete (read-only).
- **Implementato:** nessun flag. Host `net_iface` (buf 2048, retry su ENOBUFS). Blob di
  testo formattato dal kernel, stampato verbatim.
- **Manca dal reale:** non è il vero `ip` — niente subcomandi (`addr`/`link`/`route`/
  `neigh`), niente add/del/set, niente `-4`/`-6`/`-s`/`-br`/`-j`. Solo dump.

### ifconfig
- **Funzione:** mostra le interfacce, o imposta IP statico / riavvia DHCP.
- **Implementato:** no-arg → mostra tutto. `<iface>` (solo informativo, non usato per
  multiplexing — singola iface attiva). `<iface> dhcp` → `net_dhcp_renew()`.
  `<iface> <ip>/<prefix> [gw <gw>]` → `net_set_static(...)`. `parse_ip4` (dotted-quad).
  Exit 2 parse/usage, 1 errno host.
- **Manca dal reale:** multiplexing per-interfaccia non implementato (arg iface ignorato).
  Niente `up`/`down`, `netmask`/`broadcast`/`mtu`/`hw ether`, alias, IPv6. Solo IPv4.

### nc
- **Funzione:** netcat minimale, **solo client TCP**.
- **Implementato:** `<ip> <port>` (entrambi obbligatori). `parse_ip4`, `ruos::tcp_dial` →
  FD wrappato come `File`. Loop: read socket → stdout; poi legge **un byte** da stdin
  (`0x04` ^D → break, altrimenti scrive sul socket). I/O alternato, non concorrente.
- **Manca dal reale:** solo client — **niente `-l` (listen)**. Niente UDP (`-u`), `-p`
  (source port), DNS (solo IP literal), `-w` (timeout), `-z` (scan), `-e`/`-c` (exec),
  `-k`. I/O half-duplex (stdin pollato solo tra le read del socket).

### ping
- **Funzione:** ICMP echo IPv4 (solo IP literal, no DNS).
- **Implementato:** `-c N` (default 4), `-W ms` (default 1000), `<ip>`. `parse_ip4`,
  `ruos::ping(...)` per seq, stampa `seq=n time=lat ms` o `timeout`. **Sleep 1000 ms
  hardcoded** tra le probe. Statistiche finali (tx/rx/%loss/avg). Exit 1 se 0 ricevuti.
- **Manca dal reale:** niente DNS, solo IPv4. Intervallo fisso 1000 ms (no `-i`). Niente
  `-s` (size), `-t` (ttl), `-f` (flood), `-q`, `-D`, `-n`. Solo avg (no min/max/mdev).

### wget
- **Funzione:** downloader HTTP/1.0 GET minimale su file o stdout; **solo IP** (no DNS).
- **Implementato:** `-O <out>` (`-` = stdout). Default = basename del path URL o
  `index.html`. Host `tcp_dial`. `parse_url` (strip `http://`, host:port/path; host =
  IPv4 dotted stretto). Richiesta fissa `GET <path> HTTP/1.0` + Host/User-Agent/
  `Connection: close`. Legge a chunk 4096 fino a EOF; rimuove l'header fino a `\r\n\r\n`,
  scrive il body.
- **Manca dal reale:** **no DNS** (solo IPv4), **HTTP/1.0** (no HTTPS/TLS), no chunked
  decoding, **nessuna gestione status code** (ignora 404/500), no redirect (`Location`),
  no check `Content-Length`. Niente `-o`/`-a` (log), `-c` (resume), `-r`/`-m` (mirror),
  `-q`/`-nv`, `-P`, `--limit-rate`, `--timeout`, `-t`, `--header`, `--post-data`, auth,
  `-U`, IPv6, proxy, cookie, gzip, progress bar. Solo GET.

---

## Ricerca

### find
- **Funzione:** walk ricorsivo di directory che stampa i path, opz. filtrati per glob nome.
- **Implementato:** `<path>` opzionale (default `.`, solo il primo) + `-name <glob>`. Host
  `readdir` (16384). Match sul **solo leaf** del path. Glob solo `*` (split su `*`:
  starts_with/find/ends_with; senza `*` = uguaglianza esatta).
- **Manca dal reale:** glob solo `*` (no `?`, no `[abc]`). Niente predicati `-type`,
  `-iname`, `-path`, `-regex`, `-size`, `-mtime`, `-perm`, `-empty`, `-maxdepth`/
  `-mindepth`, `-prune`. Niente azioni `-exec`, `-delete`, `-print0`, `-printf`, `-ls`.
  Niente operatori booleani, root multipli, controllo symlink.

### grep
- **Funzione:** match di **sottostringa** su righe di file, stdin o alberi ricorsivi.
- **Implementato:** `-r`/`-R`/`--recursive`, `-n` (numeri riga), combinati `-rn`/`-nr`.
  Match letterale via `str::contains` (no regex). Primo non-flag = pattern; resto = path
  (nessuno → `/dev/stdin`). Prefisso `path:` solo per file/dir nominati.
- **Manca dal reale:** **nessuna regex** (no BRE/ERE/PCRE; `-E`/`-F`/`-G`/`-P` assenti).
  Niente `-i` (case), `-v` (invert), `-w`/`-x`, `-c` (count), `-l`/`-L`, `-o`, `-q`, `-s`,
  `-A`/`-B`/`-C` (context), `--color`, `-e`/`-f`, `--include`/`--exclude`, `-z`, `-m`.
  Exit status non POSIX (no exit 1 su no-match). Prefisso filename forzato anche su file
  singolo.

---

## Monitor e servizi

### rtop
- **Funzione:** monitor di sistema full-screen stile `htop` — per-core CPU%, memoria,
  uptime, tabella processi.
- **Implementato:** `--once` (output testuale plain, grep-safe). Senza flag: TUI
  interattiva via `ratatui` su `AnsiBackend` 80×24. Raw mode (`tcgetattr`/`tcsetattr`).
  Host fn `cpustat` (per-core busy/idle TSC), `proc_stat` (pid, start_tick, cpu_tsc,
  mem_bytes, name), `meminfo` (heap + frame), `uptime` (centisecondi). **Auto-refresh
  1 Hz** via `poll_stdin` (host fn, timer-driven: un tasto torna immediatamente, timeout
  ridisegna). `q` o Ctrl-C escono. Barre ASCII `[####------]` perché il font FB non ha
  glifi Unicode box-drawing. Processi ordinati per CPU% decrescente. CPU per-core: delta
  busy/(busy+idle) tra due snapshot a ~1 s. CPU per-processo: delta cpu_tsc/wall.
  Formato bytes: B/K/M. Tempo elapsed: `m:ss.cc`.
- **Manca dal reale (htop):** niente tree view, filtri, sort interattivo, kill
  interattivo, thread, `nice`, I/O counters, ricerca, mouse, resize, scroll. Geometria
  fissa 80×24. Niente colori per-usage (solo verde fisso per CPU). No signal selection.
  No swapping.

### service
- **Funzione:** gestione servizi del kernel (lista, start, status).
- **Implementato:** subcomandi posizionali: `service [list]` (default), `service start
  <name>`, `service status <name>`, `service stop <name>` (riservato, non implementato:
  exit 3). `list` stampa tabella `NAME STATUS PID RUNS PATH` via host fn
  `service_list` (buf 8192, formato TSV NUL-terminato dal kernel). `start` chiama
  `service_start` (0=ok, 1=NotFound, 2=Already, 3=NotSupported, 99=Internal). `status`
  chiama `service_status` (buf 4096, una riga TSV).
- **Manca dal reale (systemctl/service):** niente `stop`/`restart`/`enable`/`disable`,
  log, dipendenze, journal. Solo la registry kernel (shell, SSH). Nessun flag.

---

## Disco e storage

### disks
- **Funzione:** elenca i dischi SATA (indice, modello, dimensione).
- **Implementato:** nessun flag. Host `sata_list` (buf 1024, formato `<idx>\t<model>\t<size>`).
  Stampa tabella `IDX MODEL SIZE`. Il modello arriva come stringa `{:?}` (con apici);
  `trim_matches('"')` li toglie. Return `0` = nessun disco; `<0` = buffer troppo piccolo.
- **Manca dal reale (lsblk/fdisk):** nessun flag, nessun dettaglio partizioni/GPT,
  nessun filtro per tipo. Solo SATA/AHCI.

### umount
- **Funzione:** smonta un filesystem.
- **Implementato:** un operando posizionale `<path>` (obbligatorio). Host fn `ruos::umount`.
  Return `0`=ok, `-2`=non smontabile (es. `/`), `-3`=busy (file aperti), altro=non montato.
  Necessario prima di `install` quando `/mnt` è auto-montata.
- **Manca dal reale (umount):** niente `-f` (force), `-l` (lazy), `-a`, multi-path.
  Nessun parsing flag (un `-flag` sarebbe trattato come path).

### mkdisk
- **Funzione:** crea un disco ruOS: GPT + FAT32 ESP + data partition (⚠ DISTRUTTIVO).
- **Implementato:** `[esp_mib]` (default 64). Host fn `ruos::mkdisk`. Agisce sul
  **primo** disco SATA. Return `0`=ok, `-1`=nessun disco, `-2`=errore di scrittura.
- **Manca dal reale (fdisk/mkfs):** solo il primo SATA, nessuna scelta disco, nessun
  layout customizzabile. Un solo formato (GPT+FAT32).

### mkboot
- **Funzione:** come `mkdisk` + copia dell'albero di boot completo (⚠ DISTRUTTIVO).
- **Implementato:** `[esp_mib]` (default 64). Host fn `ruos::mkboot`. Agisce sul primo
  SATA. Scrive kernel, `BOOTX64.EFI`, `limine.conf` e tutti i moduli sull'ESP.
  Return come `mkdisk`.
- **Manca dal reale:** stessi limiti di `mkdisk`.

### install
- **Funzione:** installa ruOS su un disco SATA scelto (⚠ DISTRUTTIVO).
- **Implementato:** `install` senza argomenti stampa un hint (`run disks to list`).
  `install <idx> [esp_mib]` (default 64). Host fn `ruos::install`. **Guard**: rifiuta
  se `/mnt` è montato (return `-3`). Return `0`=ok, `-11`=nessun disco a quell'indice,
  `-1`=non pronto, `-2`=errore scrittura. Il disco risultante si avvia autonomamente
  sotto UEFI.
- **Manca dal reale:** nessun interattivo/conferma prima di cancellare.

---

## Compressione

### gzip
- **Funzione:** comprime file in formato gzip (RFC 1952) tramite `miniz_oxide`.
- **Implementato:** flag `-c` (stdout), `-k` (keep input), `-d` (decomprimi, come
  `gunzip`), `-1`..`-9` (livello compressione, default 6), `-h` (help). File multipli.
  Senza file: stdin→stdout. Semantica Unix: senza `-c`/`-k`, il file originale viene
  **cancellato** e rimpiazzato con `<file>.gz`. Rifiuta file che hanno già `.gz`.
  Validazione suffisso prima della lettura. Logica condivisa con `gunzip`/`zcat` nel
  crate `gzip-core` (`no_std` disponibile per il kernel).
- **Manca dal reale (gzip):** niente `--best`/`--fast` (solo flag corti), niente `-r`
  (ricorsivo), `-t` (test), `-l` (list), `-n`/`-N` (nome/timestamp), `-v` (verbose),
  `--suffix`, `--rsyncable`. Input completamente bufferizzato (non streaming). Niente
  multi-member gzip.

### gunzip
- **Funzione:** decomprime file `.gz` (= `gzip -d`).
- **Implementato:** stessi flag di `gzip`. Default: `decompress_mode=true`,
  `to_stdout=false`. Richiede suffisso `.gz` sull'input. Cancella il `.gz` dopo
  decompressione (a meno di `-k`).
- **Manca dal reale (gunzip):** stessi limiti di `gzip`.

### zcat
- **Funzione:** decomprime su stdout e tiene l'input (= `gzip -dc`).
- **Implementato:** stessi flag di `gzip`. Default: `decompress_mode=true`,
  `to_stdout=true`, `keep=true`. Non cancella mai l'input.
- **Manca dal reale (zcat):** stessi limiti di `gzip`.

---

## Wi-Fi

### wifiscan
- **Funzione:** scansiona le reti Wi-Fi 2.4 GHz nelle vicinanze via dongle USB
  RTL8188EU.
- **Implementato:** nessun flag. Host fn `ruos::wifi_scan` (buf 4096). Al **primo**
  invocamento porta il chip online (power-on + firmware + MAC/BB/RF init, ~1–2 s);
  poi esegue una scansione passiva. Stampa tabella `SSID CH SECURITY`. Return `0`=
  nessun device / nessun AP, `<0`=buffer troppo piccolo.
- **Manca dal reale (iw scan):** solo 2.4 GHz, solo passiva, solo RTL8188EU. Nessun
  filtro SSID/canale/security, nessun RSSI/signal, nessun timeout, no scan attiva.

### wificonnect
- **Funzione:** associa a una rete WPA2 via RTL8188EU.
- **Implementato:** `<ssid> [password]`. Host fn `ruos::wifi_connect`. Porta il chip
  online se non già inizializzato. Scansiona per l'SSID, poi open-system auth +
  WPA2 association (con RSN IE). Se `password` non vuota, esegue il 4-way handshake
  WPA2 (HMAC-SHA1 PTK/MIC, AES GTK unwrap) e installa le chiavi CCMP nel CAM del
  chip. Stampa una riga di stato:
  `auth=<ok|rejected|no-response> assoc=<ok|...> aid=<N> 4way=<ok|failed|skipped>`.
  Return `0`=no device, `<0`=bad args.
- **Manca dal reale (wpa_supplicant):** niente WPA3, niente WPA-Enterprise, niente
  profili salvati, niente roaming, niente DHCP (separato), niente 5 GHz. Solo
  RTL8188EU.

---

## Programmi speciali (non coreutils)

### init
- **Funzione:** primo processo userland ruos — banner di boot + sequenza di smoke test.
- **Implementato:** stampa `argv[0]`, banner ANSI "Welcome to ruos". Esegue prove con
  WASI/std standard (non host ruos): (1) sleep cooperativo 500 ms → `__wasi_poll_oneoff`;
  (2) VFS smoke: scrive `/wasm-smoke.bin`; (3) clock+random: `SystemTime::now()` +
  `getrandom`. Esce dopo i check.
- **Manca dal reale:** **non è un init reale** — niente supervisione servizi, reaping,
  respawn, signal handling PID-1, fstab/mount, runlevel. È un'harness di self-test al boot.

### client
- **Funzione:** test program client TCP via socket-activation WASI.
- **Implementato:** assume socket TCP **connesso pre-aperto su FD 4**. Import raw
  `wasi_snapshot_preview1`: `fd_write(4, "ping")` poi `fd_read(4)` (attende `"pong"`).
- **Manca dal reale:** non è un client generale — niente address/port, niente connect
  (usa FD 4), scambio singolo ping/pong. Stub di test del path socket-activation.

### server
- **Funzione:** test program server TCP via socket-activation WASI.
- **Implementato:** stampa `listening on 127.0.0.1:8080` (literal informativo, **nessun
  bind nel codice**). Socket in ascolto **pre-aperto su FD 4**: `sock_accept(4)` →
  `fd_read` → `fd_write("pong")`. Una sola connessione.
- **Manca dal reale:** single-shot (una accept/read/write poi esce). Niente accept loop,
  concorrenza, bind reale. La stringa `127.0.0.1:8080` è solo display.

---

## Pattern ricorrenti e limiti trasversali

- **Dati finti/hardcoded:** `id`, `whoami` (costanti), `lscpu` (Architecture x86_64),
  `date` (sempre "UTC"), `server` (banner 127.0.0.1:8080), `free` (`?` se heap-used n/d).
- **Retry ENOBUFS (errno 8):** solo `lspci`, `lsusb`, `ip`, `ifconfig`. `dmesg`, `ps`, `pkill`
  **non** riprovano → troncamento silenzioso al cap.
- **Parsing record binari condiviso** `ps`/`pkill` (count u32 + record pid/start/namelen/
  pad/name) e `ls`/`cp`/`rm`/`du`/`find`/`grep` (`readdir` 12-byte header).
- **Nessuna regex** in tutto il sistema (`grep`, `find` usano substring/glob `*`).
- **Nessun modello di permessi, timestamp o utenti** nel kernel: limita `touch`, `cp`,
  `ls -l`, `id`, `whoami`, `chmod`/`chown` (inesistenti).
- **Pipe `|` supportate** (max 4 stadi): i tool che leggono stdin (`grep`, `wc`, `sort`,
  `tr`, `tee`, `cut`, `uniq`, `head`, `tail`, `nc`) sono concatenabili.
  **Niente redirezioni** (`>` `<` `>>` `2>`): queste non sono ancora supportate.
- **Compressione gzip** condivisa tra `gzip`/`gunzip`/`zcat` via `gzip-core` (no_std),
  usata anche dal kernel per decomprimere i binari impacchettati.
- **Wi-Fi limitato al RTL8188EU**: `wifiscan`/`wificonnect` funzionano solo con quel
  dongle specifico; il path dati cifrato + DHCP over Wi-Fi è un lavoro in corso.
