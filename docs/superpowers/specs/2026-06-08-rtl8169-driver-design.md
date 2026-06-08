# RTL8169/8168 NIC driver — design

**Data:** 2026-06-08
**Stato:** approvato, pre-implementazione
**Branch:** `feat/usb-msc-livecd` (per richiesta utente: non creare branch nuovo)

## Contesto / motivazione

Debug su hardware reale bloccato: la seriale COM è inutilizzabile sulla macchina
di test. La via pragmatica per ottenere log remoti è **netconsole UDP** sopra lo
stack `smoltcp` già presente — ma la NIC del PC di test è una **Realtek
RTL8111/8168** (`10ec:8168`), per cui non esiste driver. ruos ha già un driver
`e1000` (Intel) con la stessa identica forma architetturale; questo lavoro
aggiunge il driver Realtek modellandolo su `e1000`.

L'infrastruttura è già predisposta:
- `NicKind::Rtl8169` esiste già (`kernel/src/net/nic/mod.rs:60`).
- Le righe della probe table per `10ec:8161/8167/8168/8169/8136` esistono già
  (`mod.rs:107-111`).
- `DescRing` (`ring.rs`) è layout-agnostico: descrittori 16 B, buffer per-slot,
  fence OWN-handoff. Riusabile senza modifiche.

Manca solo: il file driver `rtl8169.rs` + 5 punti di wiring nell'enum `Nic`.

## Scope

**In scope (MVP, parità con e1000):**
- Driver poll-mode (no IRQ) per RTL8168/8111 PCIe gigabit, device id `10ec:8168`.
- RX + TX single-buffer, riuso di `DescRing`.
- Init generico (no version-detection per-revisione stile Linux `r8169`).
- Wiring nell'enum `Nic` + dispatch in `probe_and_init`.

**Out of scope (drop espliciti):**
- IRQ/MSI-driven RX/TX (e1000 è poll-mode e basta per netconsole).
- Checksum/TSO offload.
- Tabella mac-version + quirk per-revisione (YAGNI: un solo device noto).
- RTL8139 (`NicKind::Rtl8139`, ASIC diverso) e RTL8125 2.5G (`NicKind::Rtl8125`).
- Netconsole stesso: feature separata, spec a parte. Questo lavoro porta solo
  il NIC up; netconsole verrà dopo (o si usa DHCP/ping per la verifica).

## Differenze chiave vs e1000 (rischi noti)

1. **BAR MMIO non è BAR0.** Su 8168 PCIe **BAR0 = spazio I/O, MMIO = BAR2**.
   `e1000` hardcoda BAR0; il driver Realtek deve **scansionare BAR0..5 e prendere
   il primo `Bar::Memory32/Memory64`**, non assumere l'indice.
2. **Formato descrittore diverso.** Realtek usa `opts1`/`opts2` + addr 64-bit,
   con i flag OWN/EOR/FS/LS dentro `opts1` (non un byte `status` separato come
   e1000). La lunghezza frame sta nei bit bassi di `opts1`.
3. **EOR (End Of Ring).** L'ultimo descrittore del ring DEVE avere il bit EOR
   settato (e1000 non ha questo concetto: usa RDLEN). Gestito dal driver, non da
   `DescRing`.
4. **Unlock config.** I registri di config sono read-only finché non si scrive
   `0xC0` nel registro 9346CR (0x50); si ripristina `0x00` a fine init.
5. **Kick TX esplicito.** Dopo aver pubblicato un descrittore TX, serve scrivere
   il bit NPQ nel registro TxPoll (0x38) — e1000 fa avanzare TDT invece.
6. **Direzione OWN invertita rispetto a e1000.** Su Realtek il bit OWN=1 significa
   "di proprietà del NIC"; per RX si arma OWN=1 e si attende che il chip lo
   azzeri; per TX si setta OWN=1 alla pubblicazione e il chip lo azzera a invio
   completato.

## Architettura

### Nuovo file: `kernel/src/net/nic/rtl8169.rs`

Struct speculare a `E1000`:

```rust
pub struct Rtl8169 {
    mmio: VirtAddr,   // BAR Memory mappato via map_io_range
    rx:   DescRing,
    tx:   DescRing,
    mac:  [u8; 6],
}
```

Descrittore Realtek (16 B, castato da `DescRing::slot(i)`):

```rust
#[repr(C)]
#[derive(Clone, Copy)]
struct RtlDesc {
    opts1: u32,   // OWN(31) EOR(30) FS(29) LS(28) ... frame_len(0:13/0:15)
    opts2: u32,   // VLAN (inutilizzato)
    addr:  u64,   // phys del buffer del slot
}
```

Costanti flag: `OWN = 1<<31`, `EOR = 1<<30`, `FS = 1<<29`, `LS = 1<<28`.

### Sequenza di init (`init(&mut self) -> Option<()>`)

1. `dev.enable_mmio()` + `dev.enable_bus_master()`.
2. Trova il BAR Memory: itera `dev.bar(0..6)`, prendi il primo
   `Memory32/Memory64` (su 8168 = BAR2). `map_io_range(phys, size)`.
3. Reset: CR(0x37) |= RST(0x10); poll finché RST si azzera (loop bounded come
   e1000); timeout → `None`.
4. Unlock config: scrivi `0xC0` in 9346CR(0x50).
5. Leggi MAC da IDR0..5 (offset 0x00..0x05, 6 byte).
6. C+Cmd(0xE0): config base (valore conservativo; azzera offload).
7. Inizializza i descrittori:
   - RX: per ogni slot `opts1 = OWN | (EOR se ultimo) | BUF_SIZE`, `addr = buf_phys(i)`.
   - TX: per ogni slot `opts1 = (EOR se ultimo)`, `addr = buf_phys(i)`, OWN=0
     (libero).
8. RxDescStart(0xE4 lo / 0xE8 hi) = `rx.desc_phys()`;
   TxDescStart(0x20 lo / 0x24 hi) = `tx.desc_phys()`.
   (256-byte align richiesto; le DMA region sono page-aligned → ok.)
9. MaxRxPacketSize(0xDA) = `BUF_SIZE`.
10. CR(0x37) |= TE(0x04) | RE(0x08).
11. RxConfig(0x44) = accept (AB|AM|APM|AAP) | RXFTH unlimited | MXDMA unlimited;
    TxConfig(0x40) = IFG default | MXDMA unlimited.
12. IMR(0x3C) = 0; clear ISR(0x3E) scrivendo 0xFFFF (mask IRQ — poll-mode).
13. Lock config: scrivi `0x00` in 9346CR(0x50).

### smoltcp `phy::Device`

- `receive(ts)`: leggi descrittore a `rx.head()`. Se `OWN` ancora settato → il
  chip non ha consegnato → `None`. Altrimenti verifica FS&LS (single-buffer; se
  manca, scarta+ri-arma), copia i `len` byte (da `opts1` bassi) in `Vec<u8>`,
  ri-arma il descrittore (`opts1 = OWN | EOR-se-ultimo | BUF_SIZE`), `advance_head`.
- `transmit(ts)`: sempre `Some(Rtl8169TxToken)` (queue-full gestito nel consume).
- `Rtl8169TxToken::consume(len, f)`: slot al tail SW. Se OWN ancora settato →
  coda piena → warn + drop (come e1000). Altrimenti passa il buffer DMA alla
  closure, pubblica `opts1 = OWN | FS | LS | (EOR se ultimo) | len`, `release_fence`,
  kick TxPoll(0x38) = NPQ(0x40), avanza tail SW.

Token wrapper: `pub struct Rtl8169RxToken(Vec<u8>)`,
`pub struct Rtl8169TxToken<'a>(&'a mut Rtl8169)` — speculari a e1000.

### Wiring in `kernel/src/net/nic/mod.rs` (5 punti)

1. `pub mod rtl8169;`
2. `enum Nic { E1000(..), Rtl8169(rtl8169::Rtl8169) }` + arm in `Nic::mac`.
3. `enum NicRxToken { E1000(..), Rtl8169(rtl8169::Rtl8169RxToken) }` + arm in `RxToken::consume`.
4. `enum NicTxToken<'a> { E1000(..), Rtl8169(rtl8169::Rtl8169TxToken<'a>) }` + arm in `TxToken::consume`.
5. `impl Device for Nic`: arm Rtl8169 in `capabilities`/`receive`/`transmit`;
   dispatch in `probe_and_init`:
   `NicKind::Rtl8169 => rtl8169::Rtl8169::find_and_init().map(Nic::Rtl8169)`.

`find_and_init` ri-scopre il device sul bus PCI (vendor `0x10EC`, device id tra
`0x8161/0x8167/0x8168/0x8169/0x8136`) come fa e1000.

## Error handling

Riusa `NicError`: `BarMissing` (nessun BAR Memory), `ResetTimeout` (RST non si
azzera), `Dma` (alloc ring fallita). Poll-mode → nessun path IRQ. Coda TX piena
→ drop+warn (smoltcp ritenta), come e1000.

## Testing / verifica

QEMU **non** emula RTL8168 (solo rtl8139); VBox neanche → **verifica solo su HW
reale**. Niente test automatico `make run-*-test` per questo driver.

Criteri di accettazione su HW reale:
1. Boot logga su **framebuffer** (la seriale è rotta) `found 10ec:8168 -> rtl8169`
   e `rtl8169 mac=<6 byte plausibili, non tutti 00/ff>`.
2. Link up + traffico: ruos ottiene lease DHCP **oppure** un `ping` da ruos verso
   un altro host genera pacchetti osservabili (Wireshark/tcpdump/lease su router)
   sull'altra macchina.
3. Niente regressioni: `make iso` builda, e2e su QEMU (e1000/virtio) invariato.

Build sempre via WSL: `wsl -d Ubuntu -u root -e bash -c 'cd /mnt/e/... && make iso'`.

## File toccati

- `kernel/src/net/nic/rtl8169.rs` (nuovo)
- `kernel/src/net/nic/mod.rs` (5 punti di wiring)
- `CHANGELOG/354-26-06-08-rtl8169-driver.md` (nuovo)
