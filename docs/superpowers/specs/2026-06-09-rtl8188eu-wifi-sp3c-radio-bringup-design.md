# RTL8188EU WiFi — SP3c (radio bring-up → scan) design + SP3c-1

**Data:** 2026-06-09
**Branch:** `feat/usb-wifi-rtl8188eu`
**Parent:** `2026-06-08-rtl8188eu-wifi-resources.md`
**Stato:** approvato (decomposizione + SP3c-1), pre-implementazione

## Contesto

SP1 (transport) e SP2 (power-on + firmware download + 8051 boot, `fw ready`) sono
**completi e validati su hardware reale** (CHANGELOG 388). Il firmware del chip
gira. SP3c porta su la **radio** (MAC/BB/RF) fino a uno **scan passivo** che stampa
gli SSID reali — il primo segnale radio osservabile.

Sequenza init reale mappata dal sorgente Linux mainline `rtl8xxxu`
(`rtl8xxxu_init_device` core.c:3918) tramite workflow di ricerca multi-agente.
Prerequisiti HARD per ricevere un beacon: init_mac → init_phy_bb (PHY_REG + AGC
tables) → init_phy_rf (RADIO_A table) → TRXFF_BNDY → CR MAC-RX-enable (BIT6/7,
**solo dopo** TRXFF_BNDY) → REG_RX_DRVINFO_SZ → REG_RCR (accept mgmt+bcast) →
REG_MAR=0xffffffff → FPGA0_RF_MODE (enable CCK+OFDM modems) → enable_rf →
config_channel (tune RF6052 reg 0x18).

**Decisione (utente):** il radio bring-up gira **lazy alla prima `wifiscan`**, non
nell'enumerazione USB (boot resta veloce; debug ri-lanciabile senza reboot).

## Decomposizione SP3c

| SP | Cosa | Milestone osservabile (serial/netconsole) |
|----|------|-------------------------------------------|
| **SP3c-1** | RF reg access (write/read_rfreg via LSSI) + table-apply scaffolding + udelay/mdelay sync + comando `wifiscan` (harness) | readback RF_A 0x18 plausibile (≠0/≠0xfffff) |
| SP3c-2 | Port 4 tabelle init (MAC, PHY_REG/BB, AGC, RADIO_A) in `tables.rs` | apply walk: righe + terminatori ok |
| SP3c-3 | Orchestrazione init_mac + init_phy_bb + init_phy_rf | `radio init done` + RF readback vivo |
| SP3c-4 | RX enable (TRXFF_BNDY→CR MAC-RX, RCR, MAR, DRVINFO_SZ, FPGA0_RF_MODE, enable_rf) | recv_frame torna DATI (non timeout vuoto) |
| SP3c-5 | Fix parse RxDesc (rxdesc16, drvinfo bit16-19 + shift) + de-aggregazione | primo beacon decodificato `rx bssid=.. ssid=..` |
| SP3c-6 | config_channel (RF6052 0x18) + channel-hop passivo | **`wifiscan` elenca SSID reali** 🎯 |
| SP3c-7 | comando shell `wifiscan` completo | lista AP ri-lanciabile |
| SP3c-8/9/10 | (later) calibrazione / TxDesc32 active-scan / H2C assoc | — |

Percorso minimo a scan passivo = **SP3c-1→6** (SP3c-7 nasce con SP3c-1 come harness).
Niente TX/H2C/calibrazione per lo scan passivo.

## SP3c-1 — design dettagliato

### Scope
RF register access (gli unici registri non raggiungibili dal vendor EP0 reg_read/
write: vivono dietro il BB serial LSSI 3-wire) + le primitive di supporto comuni a
tutto SP3c. Più un comando `wifiscan` minimale come harness ri-lanciabile per il
self-test (lazy: niente al boot).

### 1. Delay sincroni — `kernel/src/boot/clock.rs`
`bring_up` gira sincrono (no executor) → la `Delay` async è inusabile, e la tabella
RADIO_A ha marker delay (`0xfe`=msleep(50) ecc.). Aggiungere:
- `pub fn tsc_per_ms() -> u64` (accessor dell'esistente static `TSC_PER_MS`).
- `pub fn udelay(us: u64)`: busy-spin su `read_tsc()` per `us * tsc_per_ms()/1000` cicli.
- `pub fn mdelay(ms: u64)`: idem `ms * tsc_per_ms()`.
Entrambi bounded per costruzione (deadline TSC).

### 2. RF register access — `kernel/src/usb/wifi/mod.rs`
Costanti (FPGA0 LSSI/HSSI, path A; da `regs.h` mainline — trascrivere i valori
esatti in fase di piano):
- `REG_FPGA0_XA_HSSI_PARM1` (0x0820), `REG_FPGA0_XA_HSSI_PARM2` (0x0824),
  `REG_FPGA0_XA_LSSI_PARM` (0x0840), `REG_FPGA0_XA_LSSI_READBACK` (0x08A0),
  `REG_HSPI_XA_READBACK` (0x08B8).
- `FPGA0_LSSI_PARM_DATA_MASK = 0x000FFFFF`, `FPGA0_LSSI_PARM_ADDR_SHIFT = 20`.
- `FPGA0_HSSI_PARM2_EDGE_READ`, `FPGA0_HSSI_PARM2_ADDR_MASK/SHIFT`,
  `FPGA0_HSSI_PARM1_PI` (BIT8) — valori esatti da `regs.h` in fase di piano.

`write_rfreg(x, dev, path, reg: u8, val: u32)` (rtl8xxxu_write_rfreg verbatim):
```
let data = val & FPGA0_LSSI_PARM_DATA_MASK;
let word = ((reg as u32) << FPGA0_LSSI_PARM_ADDR_SHIFT) | data;
reg_write32(x, dev, RF_A.lssiparm, word);
clock::udelay(1);
```
Path A only per ora (8188EU è 1x1). `reg_write32` già emette LE — nessun
double-swap.

`read_rfreg(x, dev, path, reg: u8) -> u32` (rtl8xxxu_read_rfreg verbatim): toggle
EDGE_READ basso su HSSI_PARM2, set address+EDGE_READ alto, udelay, leggi PI bit da
HSSI_PARM1 → readback da `hspiread` (PI) o `lssiread` (SI), mask `& 0xFFFFF`.

### 3. Table-apply scaffolding — `kernel/src/usb/wifi/mod.rs`
Firme pronte (riempite da SP3c-2/3), con terminatori + delay-marker:
- `apply_reg8_table(x, dev, &[(u16, u8)])` — write8 per riga; stop a `(0xFFFF, 0xFF)`.
- `apply_reg32_table(x, dev, &[(u16, u32)])` — write32 + udelay(1); stop a `(0xFFFF, 0xFFFFFFFF)`.
- `apply_rf_table(x, dev, path, &[(u8, u32)])` — reg `0xFD/0xFC/0xFB/.../0xFE` = delay
  markers (mdelay) invece di scrittura; altrimenti write_rfreg + udelay(1); stop a `(0xFF, 0xFFFFFFFF)`.

### 4. Harness `wifiscan` (lazy)
Comando shell `wifiscan`: recupera il `WifiState` dalla registry USB (primo slot
`SlotKind::Wifi`), e per SP3c-1 esegue **solo** il self-test RF:
`write_rfreg(RF_A, 0x18, 0xA5A5A)` poi `read_rfreg(RF_A, 0x18)` → logga entrambi.
Cresce nei SP successivi (SP3c-6 farà lo scan vero). Re-eseguibile senza reboot.

### Deliverable + verifica (HW reale)
Dopo `fw ready`, `wifiscan` stampa `wifi: rf[0x18] wrote=0x..... read=0x.....` con
read plausibile (≠ 0x00000, ≠ 0xFFFFF) → il path LSSI raggiunge il chip RF senza
wedgare EP0. Più `wifi: tsc_per_ms=N` per sanità delay. QEMU non emula 8188EU →
validazione sul dongle reale via netconsole/seriale (come SP1/SP2). Poll bounded;
bring-up fuori dal path di boot critico → un chip wedged non blocca mai il boot.

### Scope-out (SP3c-1)
Dati delle tabelle (SP3c-2); init_mac/bb/rf (SP3c-3); RX enable (SP3c-4); fix
RxDesc (SP3c-5); config_channel/scan (SP3c-6); TxDesc32/H2C/calibrazione (SP3c-8+).

## File toccati (SP3c-1)
- `kernel/src/boot/clock.rs` (tsc_per_ms + udelay + mdelay)
- `kernel/src/usb/wifi/mod.rs` (LSSI consts + write/read_rfreg + apply_* scaffolding + RF self-test)
- shell command `wifiscan` (file shell builtins — da individuare in fase di piano) + registry retrieval
- `CHANGELOG/NN-26-06-09-wifi-sp3c1-rf-access.md`

## Rischi (dal workflow)
- RF access è indiretto (LSSI): shift/mask 20-bit sbagliati = scrive garbage nel chip RF, **nessun errore** → radio sorda difficile da debuggare. Il self-test readback esiste per beccarlo subito.
- Il blocco dominante è BB/AGC/RF (SP3c-2/3), non il datapath: fixare descrittori/RCR da soli mostra zero beacon finché la radio è fisicamente spenta.
- Ordine trap: CR MAC-RX bit solo dopo TRXFF_BNDY (SP3c-4).
- TxDesc 8188eu = 32B (non 40B), RxDesc parse drvinfo bit16-19 (non >>24) — fix in SP3c-5/9.
