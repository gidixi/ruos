//! USB HID boot keyboard.

/// A detected HID boot keyboard's interrupt-IN endpoint.
#[derive(Clone, Copy)]
pub struct HidKeyboard {
    pub iface:      u8,   // bInterfaceNumber
    pub ep_addr:    u8,   // bEndpointAddress (bit7=IN, low4=EP number)
    pub max_packet: u16,  // wMaxPacketSize
    pub interval:   u8,   // bInterval
}
