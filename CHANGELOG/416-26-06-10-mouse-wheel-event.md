# Mouse wheel → gfx-event kind 5 → wm.poll_event

The wheel was decoded nowhere and the compositor had no event kind for it; GUI
apps (notably the Blitz viewer, Phase 3 scroll) could not receive scroll input.

- `mouse/mod.rs`: `MouseEvent.wheel: i16` (detents, positive = scroll up / away
  from user). PS/2 IntelliMouse unlock at init (sample-rate 200,100,80 + Get-ID;
  ID 3 → 4-byte packets, byte3 = Z, negated since PS/2 reports toward-user as
  positive — QEMU answers ID 3). ISR assembles 3- or 4-byte packets per mode.
  `decode_packet4` + self-test cases.
- `usb/mouse.rs`: HID boot-mouse byte 3 decoded as wheel (already positive=up,
  no negation) + self-test cases.
- `gfx/mod.rs`: gfx-event **kind 5 wheel** `{p0 = detents i32}`; `fold_mouse`
  emits it.
- `wt/wm.rs` run loop: kind 5 routed to the topmost window under the cursor
  (hover-scroll, bg fallthrough), preceded by a window-local MouseMove.
- `ruos-desktop` (gui-core + ruos-window): `GfxEvent::Wheel { dy }` decoded from
  kind 5 → `egui::Event::MouseWheel` (Line unit). Old apps ignore unknown kinds
  (`_ => continue`) — no re-AOT needed; engine config untouched.
- `docs/api/wm.md`: kind table + routing note; Last reviewed bumped.
