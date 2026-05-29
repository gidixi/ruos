# 139 — Task 4: NIC iface wiring + Makefile NIC param + e1000 gate

**Data:** 2026-05-29

## Cosa

Task 4 dello spec. Wire del driver e1000 (Task 3) nello smoltcp
NetState + parametro Makefile per scegliere il modello QEMU + gate
DHCP dedicato.

### kernel/src/net/mod.rs

Dual-field approach: `dev_net: Option<virtio::VirtioNet>` +
`dev_nic: Option<nic::Nic>` con ifaces separate. Al più uno dei
due è attivo alla volta (xor) — virtio preferito dentro VM,
fallback su prima NIC hardware (e1000 per ora) dalla `nic` probe
table.

Init flow:
1. Prova `virtio::VirtioNet::find_and_init()` → se Some, popola
   `dev_net` + `iface_net` + DHCP socket
2. Else prova `nic::probe_and_init()` → se Some, popola `dev_nic` +
   `iface_nic` + DHCP socket
3. Altrimenti bwarn loopback only

Poll loop polla entrambe le ifaces (al più una con device → l'altro
ramo no-op). Gestione DHCP event applicata alla iface presente
(`iface_net.or_else(|| iface_nic)`).

Approccio NetDevice enum tentato prima → causava hang virtio init
durante boot (cause exact unidentified, prob. mono-bloat o reorder).
Dual-field = cambio minimo + smoltcp vede tipo concreto a compile
time, no enum dispatch.

### kernel/src/wasm/host/proc.rs

`ruos_net_iface` ora controlla virtio xor nic. Output `eth0  IP/cidr
mac=... gw=...` invariato per il consumer.

### Makefile

- `NIC ?= virtio-net-pci` (default), override `make run NIC=e1000`
- Target `run-test-e1000`: chiama `run-test NIC=e1000` + asserisce
  `net: e1000 mac=...` in serial.log → `TEST_PASS_E1000`

## Test

Doppio gate:

`make run-test` (default virtio): TEST_PASS — virtio + DHCP gates
preservati.

`make run-test-e1000`: TEST_PASS_E1000. Serial mostra:
```
[T+4.297s] INFO nic  found 8086:100e -> e1000 (bus=0 dev=3 fn=0)
[T+4.466s] INFO net  e1000 mac=[52, 54, 00, 12, 34, 56]
[T+6.700s] INFO net  dhcp bound ip=10.0.2.15 gw=10.0.2.2
```

`ip` in shell mostra:
```
lo    127.0.0.1/8
eth0  10.0.2.15/24 mac=52:54:00:12:34:56 gw=10.0.2.2
```

## File toccati

- kernel/src/net/mod.rs (dual dev_net/dev_nic + dual iface)
- kernel/src/wasm/host/proc.rs (ruos_net_iface virtio xor nic)
- Makefile (NIC param + run-test-e1000 target)
- CHANGELOG/139-26-05-29-net-nic-wiring-e1000-gate.md (questo)
