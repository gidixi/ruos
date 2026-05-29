# 137 — Task 2: shared NIC descriptor-ring engine

**Data:** 2026-05-29

## Cosa

`kernel/src/net/nic/ring.rs` (`DescRing`): engine condiviso per ring
descriptor di tutti i driver NIC. Allocazione un `DmaRegion` per i
descriptor + N `DmaRegion` 4 KiB (uno per slot) per i packet buffer.

- `SLOT_SIZE = 16` — descrittore legacy (e1000, RTL8169) e advanced
  (igb/igc split read/writeback) sono entrambi 16 byte
- `BUF_SIZE = 2048` — Ethernet frame full 1518 byte + slack VLAN/FCS
- `DescRing::new(count) -> Option<Self>`: alloc atomico (rollback se
  fallisce mid-way)
- `desc_phys/virt`, `slot(i)`, `buf_phys/virt(i)`: accessor per il
  driver che casta lo slot al suo `#[repr(C)]` struct
- `head`/`advance_head` per indice software
- `release_fence`/`acquire_fence`: `compiler_fence(SeqCst)` (x86 ha
  store-ordering forte → solo barrier compiler necessario)
- `Drop` libera tutti i `DmaRegion`

Memoria DMA = RAM cacheable normale (x86 DMA-coherent, no NO_CACHE
flag), come `kernel/src/memory/dma.rs` documenta.

## Perché

Tutti i driver NIC (e1000, e1000e, igb, igc, rtl8169/8125) condividono:
- layout ring + descriptor 16 B
- semantica OWN/EOR + indici head/tail
- fence sequence pre-MMIO-write

Estrarre nel modulo evita 5× duplicazione. Driver scrive solo i campi
chip-specific via volatile sul `slot()`.

RTL8139 e tg3 NON usano questo engine (register-based o status block
+ producer/consumer locali) — coerente con spec.

## Test

`make build` → `Finished` (53 warnings preesistenti, 4 nuovi
dead-code "field never read" su `DescRing.bufs`/`count` — utilizzo
quando Task 3 implementa e1000).

## File toccati

- kernel/src/net/nic/ring.rs (nuovo, 135 righe)
- kernel/src/net/nic/mod.rs (`pub mod ring;`)
- CHANGELOG/137-26-05-29-nic-ring-engine.md (questo)
