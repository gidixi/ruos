//! Minimal CMOS RTC reader (I/O ports 0x70/0x71).
//!
//! Polling: wait for the Update-In-Progress flag to clear, then read every
//! field twice and accept the read if both attempts agree. Handles BCD or
//! binary mode + 12/24-hour transparently.

use x86_64::instructions::port::Port;

#[derive(Debug, Clone, Copy)]
pub struct RtcTime {
    pub year:   u16,
    pub month:  u8,
    pub day:    u8,
    pub hour:   u8,
    pub minute: u8,
    pub second: u8,
}

fn read_reg(reg: u8) -> u8 {
    unsafe {
        Port::<u8>::new(0x70).write(reg);
        Port::<u8>::new(0x71).read()
    }
}

fn uip_set() -> bool { (read_reg(0x0A) & 0x80) != 0 }

fn bcd_to_bin(v: u8) -> u8 { ((v >> 4) * 10) + (v & 0x0F) }

pub fn now() -> RtcTime {
    // Wait for UIP to clear.
    let mut spin = 0u32;
    while uip_set() {
        spin += 1;
        if spin > 1_000_000 { break; }
        core::hint::spin_loop();
    }

    let read_once = || -> RtcTime {
        let sec  = read_reg(0x00);
        let min  = read_reg(0x02);
        let hour = read_reg(0x04);
        let day  = read_reg(0x07);
        let mon  = read_reg(0x08);
        let yr   = read_reg(0x09);
        let regb = read_reg(0x0B);

        let bcd  = (regb & 0x04) == 0;
        let h24  = (regb & 0x02) != 0;

        let sec  = if bcd { bcd_to_bin(sec) } else { sec };
        let min  = if bcd { bcd_to_bin(min) } else { min };
        let mut h = hour & 0x7F;
        let pm    = (hour & 0x80) != 0;
        h         = if bcd { bcd_to_bin(h) } else { h };
        if !h24 {
            if pm && h < 12 { h += 12; }
            if !pm && h == 12 { h = 0; }
        }
        let day  = if bcd { bcd_to_bin(day) } else { day };
        let mon  = if bcd { bcd_to_bin(mon) } else { mon };
        let yr   = if bcd { bcd_to_bin(yr) } else { yr } as u16;
        // Assume 21st century; firmware century register varies.
        let year = 2000 + yr;
        RtcTime { year, month: mon, day, hour: h, minute: min, second: sec }
    };

    // Double-read until two consecutive reads match (guards against rollover).
    let mut a = read_once();
    for _ in 0..3 {
        let b = read_once();
        if a.year == b.year && a.month == b.month && a.day == b.day
            && a.hour == b.hour && a.minute == b.minute && a.second == b.second
        {
            return a;
        }
        a = b;
    }
    a
}

/// Convert an `RtcTime` (assumed UTC) to seconds since 1970-01-01.
pub fn to_unix_epoch(t: &RtcTime) -> u64 {
    // Days from 1970-01-01 to year-month-day. Simple Gregorian conversion.
    fn is_leap(y: u16) -> bool {
        (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0)
    }
    const MDAYS: [u32; 12] = [31,28,31,30,31,30,31,31,30,31,30,31];

    let mut days: u64 = 0;
    for y in 1970..t.year {
        days += if is_leap(y) { 366 } else { 365 };
    }
    let mi = (t.month - 1) as usize;
    for m in 0..mi {
        days += u64::from(MDAYS[m]);
        if m == 1 && is_leap(t.year) { days += 1; }
    }
    days += u64::from(t.day - 1);
    days * 86400 + u64::from(t.hour) * 3600 + u64::from(t.minute) * 60 + u64::from(t.second)
}
