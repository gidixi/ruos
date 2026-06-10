# gzip / gunzip / zcat — tool di compressione userland

**Data:** 2026-06-10
**Stato:** approvato (brainstorming → design ok)

## Obiettivo

ruos non ha alcun sistema di compressione. Si aggiungono i tre comandi classici
`gzip`, `gunzip`, `zcat` come tool WASM `wasm32-wasip1` eseguiti da wasmi, con
formato gzip (RFC 1952) + deflate (RFC 1951) via `miniz_oxide` (pure Rust).

Scelte fatte in brainstorming:

- **Formato: gzip/deflate** (non xz/zstd/lz4): `miniz_oxide` è pure Rust,
  compress+decompress completi, compila su wasm32-wasip1 senza problemi,
  formato più diffuso (`.gz`, `tar.gz`).
- **Comandi: gzip + gunzip + zcat** — trio classico, componibile con le pipe
  della shell esistente.
- **Semantica: Unix classica** — `gzip f` crea `f.gz` e cancella `f`;
  `gunzip f.gz` ricrea `f`; senza argomenti stdin→stdout.

## Architettura (approccio A: tre crate, core condiviso)

```
user/
  gzip-core/   lib condivisa: formato gzip + compress/decompress (miniz_oxide)
  gzip/        bin: CLI completa (-c -k -d -1..-9 -h)
  gunzip/      bin thin: = gzip -d
  zcat/        bin thin: = gzip -dc
```

Approcci scartati:

- **Multi-call binary stile busybox** (dispatch su argv[0]): la shell risolve
  `/bin/<nome>.wasm` per nome file e tmpfs non ha symlink → servirebbero
  comunque 3 copie. Risparmio zero.
- **`flate2` (backend rust)**: wrapper su miniz_oxide; dipendenza in più per
  poco, i tool esistenti tengono le dipendenze minime.

### `user/gzip-core` (lib)

- **Compress**: `miniz_oxide::deflate::compress_to_vec` + header gzip
  (magic `1f 8b`, CM=8, FLG=0, MTIME=0, XFL, OS=255) + trailer CRC32 + ISIZE.
  CRC32 implementato nella lib (tabella 256 voci) — miniz_oxide non lo espone.
- **Decompress**: parsing header (gestione FLG: FEXTRA/FNAME/FCOMMENT/FHCRC
  da saltare correttamente), `miniz_oxide::inflate::decompress_to_vec` sul
  payload, verifica CRC32 e ISIZE del trailer.
- **Fuori scope**: file gzip multi-member (più stream concatenati) — si
  decodifica solo il primo member; byte residui dopo il trailer → errore
  TrailingGarbage. Estensione futura se serve.
- **Errori tipizzati** (`enum GzError`): NotGzip, TruncatedHeader,
  TruncatedTrailer, BadDeflate, CrcMismatch, SizeMismatch, TrailingGarbage.
  `Display` per i messaggi CLI.
- **API**:
  - `compress(data: &[u8], level: u8) -> Vec<u8>` (level 1..=9, default 6)
  - `decompress(data: &[u8]) -> Result<Vec<u8>, GzError>`
  - `run_cli(default_decompress: bool, default_stdout: bool)` — entrypoint
    condiviso dai tre bin: parsing flag + dispatch file/stdin.

### CLI (`run_cli` in gzip-core, riusata dai tre bin)

| Tool   | Equivale a | Default                         |
|--------|-----------|----------------------------------|
| gzip   | —         | comprimi                         |
| gunzip | gzip -d   | decomprimi                       |
| zcat   | gzip -dc  | decomprimi su stdout, keep input |

Flag: `-c` (stdout, non tocca file), `-k` (keep originale), `-d` (decomprimi),
`-1`..`-9` (livello compressione), `-h` (usage). Combinabili (`-dc`, `-kc`).

Comportamento file-mode:

- `gzip f` → scrive `f.gz`, cancella `f`. Se `f` finisce già in `.gz`: errore
  "already has .gz suffix" (exit 1, skip).
- `gunzip f.gz` → scrive `f` (suffisso rimosso), cancella `f.gz`. Se il nome
  non finisce in `.gz`: errore "unknown suffix".
- Output già esistente → errore, niente overwrite silenzioso (no `-f`, YAGNI).
- Più file in argv: processati in sequenza, errore su uno non blocca i
  successivi; exit code 1 se almeno uno fallisce.
- Senza file: stdin→stdout (in questo caso `-c` implicito).

I/O: file letti interi in RAM (stile `cat` esistente); limite pratico = heap
del guest wasmi. Adeguato per file su tmpfs/FAT32 di dimensioni ragionevoli.

### Stile bin

Come i tool esistenti: `ruos_rt::init()` a inizio main (sync cwd), errori
`eprintln!("gzip: {}: {}", path, e)` + `std::process::exit(1)`.

## Wiring build

- `user/Cargo.toml`: + membri `gzip-core`, `gzip`, `gunzip`, `zcat`.
- `Makefile`: + `gzip gunzip zcat` in `BIN_TOOLS` (la pattern rule
  `user-bin/%.wasm` copre il resto). NB: la pattern rule non traccia
  `gzip-core/src/lib.rs` come dipendenza make — cargo ricompila comunque
  correttamente; stesso limite accettato per `ruos-rt`.
- Niente kernel, niente host fn nuove → `docs/api/` non si tocca.

## Error handling (riepilogo)

- Input non gzip / troncato / CRC o ISIZE errati → messaggio specifico, exit 1.
- File inesistente / permessi → errore I/O standard, exit 1.
- Argomenti sconosciuti → usage su stderr, exit 1.

## Test

- **Unit test in `gzip-core`** (girano su host, `cargo test -p gzip-core`):
  - roundtrip: compress→decompress = identità (vuoto, piccolo, ~1 MiB random,
    testo ripetitivo) a livelli 1/6/9;
  - header golden: output compress inizia con `1f 8b 08 00`;
  - decompress di un vettore `.gz` noto (creato con gzip reale, embedded);
  - header con FNAME/FEXTRA → skip corretto;
  - corruzione CRC → `CrcMismatch`; payload troncato → errore, niente panic;
  - suffix logic: nomi output corretti, rifiuto doppia compressione.
- **Smoke su ruos** (manuale, `make run`): `echo hi | gzip | gunzip`,
  `gzip /tmp/f && gunzip /tmp/f.gz`, `zcat f.gz`.
