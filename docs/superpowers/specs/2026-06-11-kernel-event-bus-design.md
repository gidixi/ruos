# Kernel event bus + notifiche compositor (v1) — Design

**Data:** 2026-06-11
**Stato:** approvato (brainstorming), da pianificare — NON ancora implementato

## Obiettivo

Sistema di notifiche kernel→compositor in stile pub/sub: il kernel pubblica
eventi strutturati su un bus broadcast (ring buffer), il compositor li consuma
e li rende visibili all'utente (toast / modale). Casi d'uso iniziali: app WASM
crashata o fuori fuel, spegnimento/riavvio imminente, memoria fisica in
esaurimento.

Scelta architetturale di fondo (modello **ibrido**): il rendering kernel-side
(modulo `decor`) garantisce le notifiche critiche anche se il desktop egui è
morto o assente; il ring broadcast con cursori per-lettore rende gratuita una
futura API app-facing (`sys.events_poll`) per notifiche ricche, **fuori scope
in v1**.

Principi (dal brainstorming):

- **Kernel = eventi, compositor = policy.** Il kernel dice `APP_CRASHED`; è il
  compositor che decide toast vs modale in base alla severity.
- **Publish IRQ-safe, zero alloc.** Pubblicare = scrivere uno slot nel ring +
  incrementare seq. Mai bloccare, mai allocare.
- **Seq number monotonico** → un lettore lento rileva da solo il gap e
  sintetizza `SUBSCRIBER_OVERFLOW`. Nessuna registrazione subscriber in v1.
- **Niente demone intermedio** (`notifyd`): il compositor è già kernel-side,
  è lui l'hub. Niente `/dev/kevents` in v1 (possibile v2 per tool wasmi/SSH).
- **L'enforcement dello shutdown non dipende dalla UI**: il modale è solo
  visualizzazione, il timeout scatta comunque.

## 1. Bus — modulo `kernel/src/kevent.rs`

### Formato evento

```rust
#[repr(C)]
#[derive(Clone, Copy)]
pub struct KEvent {
    pub seq: u64,          // monotonico globale, parte da 1
    pub kind: u16,         // topic: byte alto = categoria (vedi catalogo)
    pub severity: u8,      // 0 = INFO, 1 = WARN, 2 = CRIT
    pub _pad: u8,
    pub ts_ticks: u32,     // tick timer 100 Hz al momento del publish
    pub payload: [u32; 4], // semantica per-kind
}
```

Struct fissa, versionata implicitamente dal `kind`: kind nuovi = ID nuovi,
mai riusare/ridefinire il payload di un kind esistente.

### Storage

- Ring statico: `IrqMutex<[KEvent; RING_LEN]>` con `RING_LEN = 64`, più
  contatore `seq` (`AtomicU64`). Slot di scrittura = `seq % RING_LEN`
  (sovrascrittura circolare). Stesso pattern di `gfx::EVENTS` / klog ring.
- **Side-table stringhe**: i nomi app non entrano nel payload fisso. Tabella
  circolare parallela `[heapless::String<32>; RING_LEN]`, stesso indice dello
  slot. Publish con nome = copia troncata nella side-table, zero alloc.

### API

```rust
pub fn publish(kind: u16, severity: u8, payload: [u32; 4]);
pub fn publish_named(kind: u16, severity: u8, payload: [u32; 4], name: &str);

/// Lettura da cursore: copia in `out` gli eventi con seq > last_seq,
/// ritorna (n_copiati, lost). `lost > 0` se il ring ha sovrascritto eventi
/// non ancora letti (gap = seq_globale - last_seq - RING_LEN, se positivo);
/// in quel caso il lettore sintetizza localmente SUBSCRIBER_OVERFLOW{lost}.
pub fn read_since(last_seq: u64, out: &mut [KEvent]) -> (usize, u64);
pub fn name_of(seq: u64) -> Option<heapless::String<32>>; // valida solo se slot non sovrascritto
```

Ogni lettore tiene il proprio cursore `last_seq`; il bus non sa chi legge.
In v1 l'unico lettore è il compositor.

## 2. Catalogo eventi v1

Categorie nel byte alto del `kind`: `0x00` meta-bus, `0x01` power,
`0x02` app/risorse. Riservati per fase 2: `0x03` storage, `0x04` hotplug/net.

| Kind | Valore | Sev | Payload | Pubblicato da |
|---|---|---|---|---|
| `SUBSCRIBER_OVERFLOW` | 0x0001 | INFO | `[lost_lo, lost_hi, 0, 0]` | sintetizzato dal lettore (mai nel ring) |
| `SHUTDOWN_PENDING` | 0x0101 | CRIT | `[countdown_sec, reason, 0, 0]` | `power.rs` su `request_poweroff` |
| `REBOOT_PENDING` | 0x0102 | CRIT | `[countdown_sec, reason, 0, 0]` | `power.rs` su `request_reboot` |
| `POWER_CANCELLED` | 0x0103 | INFO | `[0; 4]` | `power.rs` su `cancel()` |
| `APP_CRASHED` | 0x0201 | WARN | `[win_id, causa, 0, 0]` + nome in side-table | `wm.rs`, path d'errore di `frame()` (oggi ~riga 1550) |
| `APP_FUEL_EXHAUSTED` | 0x0202 | WARN | `[pid, 0, 0, 0]` + nome | error path out-of-fuel del runtime wasmi |
| `MEM_LOW` | 0x0203 | WARN | `[frame_liberi, frame_totali, 0, 0]` | frame allocator |

- `APP_CRASHED.causa`: `0` = trap WASM, `1` = epoch watchdog deadline,
  `2` = memoria/instantiate fallita.
- `SHUTDOWN/REBOOT_PENDING.reason`: `0` = richiesta utente (unico in v1).
- `MEM_LOW`: soglia frame liberi < 10% del totale, con **isteresi** — dopo il
  publish si ri-arma solo quando i liberi risalgono sopra il 15%. Un evento
  per attraversamento, niente spam.

## 3. Shutdown/reboot differito annullabile

`kernel/src/power.rs` guadagna stato e API:

```rust
pub enum PendingKind { Poweroff, Reboot }
struct Pending { kind: PendingKind, deadline_tick: u64 }
static PENDING: IrqMutex<Option<Pending>>;

pub fn request_poweroff(countdown_sec: u32);
pub fn request_reboot(countdown_sec: u32);
pub fn cancel();
pub fn pending() -> Option<(PendingKind, u64 /* tick rimanenti */)>;
```

- `request_*`: setta `PENDING`, pubblica `*_PENDING` sul ring, spawna un task
  sull'executor async che dorme fino a `deadline_tick`; al risveglio, se
  `PENDING` è ancora set, chiama `power::poweroff()` / `reboot()` veri
  (never-return). Richiesta duplicata mentre PENDING attivo = no-op.
- `cancel()`: clear `PENDING` + publish `POWER_CANCELLED`. Il task in volo
  trova `PENDING == None` e termina senza spegnere.
- **L'enforcement è il task, non il compositor**: lo spegnimento avviene anche
  headless o con compositor morto. Il modale è solo UI.

### Cambio semantica host fn (ABI)

`wm.poweroff()` e `wm.reboot()` **cambiano**: da never-return immediato a
"richiesta differita che ritorna" (`request_*(10)` e return al guest). Stesso
nome, stessa assenza di parametri, ma la signature logica non è più `-> !`.
Da aggiornare **nello stesso commit** (regola CLAUDE.md):

- `docs/api/wm.md` (semantica + "Last reviewed");
- `ruos-desktop/crates/ruos-window/src/lib.rs` (extern "C": le fn non sono
  più divergenti).

## 4. Rendering nel compositor

Nuovo step `drain_kevents()` nel loop `Compositor::run()`, dopo la fase input:
`read_since(cursor)` → smista per severity.

### Toast (INFO / WARN)

- Stato `Compositor`: `toasts: Vec<Toast>` con
  `Toast { kind, text: String, born_tick, sev }` (qui l'alloc è lecita:
  contesto compositor, non IRQ).
- Stack in alto a destra, **max 3 visibili**, gli altri in coda FIFO.
- Vita **~5 s** (500 tick), poi scarto.
- Disegno in `present()` col modulo `decor` (`fill_rect` + `draw_text`):
  sopra le finestre composite, **sotto il cursore software**. Bordo colorato
  per severity: INFO grigio, WARN ambra.
- Click su un toast = dismiss immediato (hit-test toast **prima** di quello
  finestre nel routing input).
- Gap rilevato da `read_since` → toast `SUBSCRIBER_OVERFLOW` ("persi N eventi").

### Modale (CRIT: `SHUTDOWN_PENDING` / `REBOOT_PENDING`)

- Rettangolo centrato: titolo ("Spegnimento" / "Riavvio"), testo
  "tra N s", bottone **Annulla**.
- Mentre il modale è attivo: input mouse/tastiera routato al modale, **non**
  alle finestre. `Esc` o click su Annulla → `power::cancel()`.
- Countdown ridisegnato ogni secondo leggendo `power::pending()` (fonte di
  verità: lo stato PENDING, non l'evento).
- Il modale si chiude su `POWER_CANCELLED` o quando `pending()` torna `None`.

Toast e modale sono stato del `Compositor`, nessuna finestra WASM coinvolta.

## 5. Fuori scope v1 (deciso, non dimenticato)

- Host fn app-facing `sys.events_poll` / subscribe con maschera topic (v2 —
  il ring con cursori la supporta già; andrà documentata in `docs/api/`).
- Device file `/dev/kevents` per tool wasmi / sessioni SSH headless.
- Eventi fase 2: `NET_LINK_UP/DOWN`, `DHCP_BOUND`, `SSH_LOGIN`,
  `USB_DEVICE_ADDED/REMOVED`, `STORAGE_IO_ERROR`, `DISK_SPACE_LOW`.
- Suoni, centro notifiche, persistenza notifiche.

## 6. Test e verifica

- `make run-test` deve restare verde (nessuna regressione boot).
- Con `CARGO_FEATURES=boot-checks`: self-test del ring — publish di
  `RING_LEN + 6` eventi, verifica ordine seq e che `read_since` da cursore
  vecchio riporti `lost == 6`.
- Verifica visiva in `make run`: builtin shell di debug (feature-gated o
  rimovibile) `kev-test` che pubblica un evento WARN di prova → toast a
  schermo; shutdown/reboot testabili dal desktop (modale + Annulla +
  spegnimento effettivo allo scadere).
- Caso negativo: countdown scade con compositor assente (console mode) →
  la macchina si spegne comunque (enforcement = task async).

## Parametri (default v1)

| Parametro | Valore |
|---|---|
| `RING_LEN` | 64 |
| Countdown shutdown/reboot | 10 s |
| Vita toast | 5 s |
| Toast visibili max | 3 |
| Soglia `MEM_LOW` | < 10% frame liberi (isteresi ri-arma > 15%) |
| Side-table nome | `heapless::String<32>` |
