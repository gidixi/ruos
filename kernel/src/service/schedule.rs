//! Schedule dei timer unit: parse della stringa schedule e calcolo del
//! prossimo scatto. Vedi spec init-units §1/§4.

use alloc::string::{String, ToString};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Schedule {
    /// Intervallo monotono in tick (OnUnitActiveSec-like).
    EveryTicks(u64),
    /// One-shot a boot+N tick (si disabilita dopo lo scatto).
    BootPlus(u64),
    Hourly { minute: u8 },
    Daily  { hour: u8, minute: u8 },
    Weekly { dow: u8, hour: u8, minute: u8 },   // 0=Sun..6=Sat
}

/// "hourly :MM" | "daily HH:MM" | "weekly Dow HH:MM" | "every Ns" | "boot+Ns"
/// Tick = 10 ms (timer 100 Hz): secondi*100.
pub fn schedule_parse(s: &str) -> Result<Schedule, String> {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix("every ") {
        return Ok(Schedule::EveryTicks(secs(rest)? * 100));
    }
    if let Some(rest) = s.strip_prefix("boot+") {
        return Ok(Schedule::BootPlus(secs(rest)? * 100));
    }
    if let Some(rest) = s.strip_prefix("hourly ") {
        let m = rest.trim().strip_prefix(':').ok_or_else(|| "hourly: want :MM".to_string())?;
        let minute = num(m, 59)? as u8;
        return Ok(Schedule::Hourly { minute });
    }
    if let Some(rest) = s.strip_prefix("daily ") {
        let (h, m) = hhmm(rest)?;
        return Ok(Schedule::Daily { hour: h, minute: m });
    }
    if let Some(rest) = s.strip_prefix("weekly ") {
        let (dow_s, hm) = rest.trim().split_once(' ')
            .ok_or_else(|| "weekly: want 'Dow HH:MM'".to_string())?;
        let dow = match dow_s.to_ascii_lowercase().as_str() {
            "sun" => 0, "mon" => 1, "tue" => 2, "wed" => 3,
            "thu" => 4, "fri" => 5, "sat" => 6,
            _ => return Err("weekly: bad day".to_string()),
        };
        let (h, m) = hhmm(hm)?;
        return Ok(Schedule::Weekly { dow, hour: h, minute: m });
    }
    Err(alloc::format!("unknown schedule '{}'", s))
}

fn secs(s: &str) -> Result<u64, String> {
    let s = s.trim().strip_suffix('s').ok_or_else(|| "want Ns".to_string())?;
    s.parse::<u64>().map_err(|_| "bad number".to_string())
}

fn hhmm(s: &str) -> Result<(u8, u8), String> {
    let (h, m) = s.trim().split_once(':').ok_or_else(|| "want HH:MM".to_string())?;
    Ok((num(h, 23)? as u8, num(m, 59)? as u8))
}

fn num(s: &str, max: u32) -> Result<u32, String> {
    let v = s.trim().parse::<u32>().map_err(|_| "bad number".to_string())?;
    if v > max { return Err(alloc::format!("{} out of range", v)); }
    Ok(v)
}

/// Prossimo scatto FUTURO. Calendario: input/output = unix epoch (s).
/// Monotoni: input/output = tick (EveryTicks→now_ticks+n; BootPlus→n, armato
/// una volta sola al load). Pura: testabile nei boot-check. Day-of-week da
/// epoch: 1970-01-01 = giovedì = 4.
pub fn compute_next(s: &Schedule, epoch_now: u64, now_ticks: u64) -> u64 {
    const HOUR: u64 = 3_600;
    const DAY:  u64 = 86_400;
    match *s {
        Schedule::EveryTicks(n) => now_ticks + n,
        Schedule::BootPlus(n)   => n,
        Schedule::Hourly { minute } => {
            let cand = epoch_now - epoch_now % HOUR + u64::from(minute) * 60;
            if cand > epoch_now { cand } else { cand + HOUR }
        }
        Schedule::Daily { hour, minute } => {
            let cand = epoch_now - epoch_now % DAY
                     + u64::from(hour) * HOUR + u64::from(minute) * 60;
            if cand > epoch_now { cand } else { cand + DAY }
        }
        Schedule::Weekly { dow, hour, minute } => {
            let days = epoch_now / DAY;
            let dow_now = ((days + 4) % 7) as u8;
            let delta = u64::from((dow + 7 - dow_now) % 7);
            let cand = (days + delta) * DAY
                     + u64::from(hour) * HOUR + u64::from(minute) * 60;
            if cand > epoch_now { cand } else { cand + 7 * DAY }
        }
    }
}

/// Backoff esponenziale capato: 1s,2s,4s,…,30s (in tick @100Hz).
/// `restarts` = numero di restart già fatti (0-based al primo).
pub fn backoff_ticks(restarts: u32) -> u64 {
    core::cmp::min(100u64 << restarts.min(5), 3_000)
}
