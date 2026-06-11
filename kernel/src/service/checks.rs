//! Self-test init-units, gated `boot-checks`. Panica su mismatch (il
//! run-test QEMU fallisce visibilmente); su pass logga una riga per gruppo.
use alloc::string::ToString;

pub fn run() {
    check_yaml();
    check_json();
    check_schedule();
    check_unitfile();
    check_compute_next();
    crate::binfo!("svc-check", "init-units checks OK");
}

fn check_compute_next() {
    use super::schedule::{compute_next, Schedule};
    // 2026-06-08 14:30:00 UTC = 1780929000 (lunedì, dow=1) — verificato con
    // `date -u -d @N`.
    let now: u64 = 1_780_929_000;
    assert_eq!(compute_next(&Schedule::Daily { hour: 3, minute: 0 }, now, 0),
               1_780_974_000);              // 2026-06-09 03:00:00 (domani)
    assert_eq!(compute_next(&Schedule::Hourly { minute: 0 }, now, 0),
               1_780_930_800);              // 15:00:00
    assert_eq!(compute_next(&Schedule::Hourly { minute: 45 }, now, 0),
               1_780_929_900);              // 14:45:00 (stessa ora, futuro)
    // weekly Tue 14:00 → domani: 2026-06-09 14:00:00
    assert_eq!(compute_next(&Schedule::Weekly { dow: 2, hour: 14, minute: 0 }, now, 0),
               1_781_013_600);
    // weekly Mon 09:30 → oggi è lunedì ma orario passato → +7g: 2026-06-15 09:30:00
    assert_eq!(compute_next(&Schedule::Weekly { dow: 1, hour: 9, minute: 30 }, now, 0),
               1_781_515_800);
    // rollover anno: 2026-12-31 23:59:00 → daily 00:00 = 2027-01-01 00:00:00
    assert_eq!(compute_next(&Schedule::Daily { hour: 0, minute: 0 }, 1_798_761_540, 0),
               1_798_761_600);
    // monotoni: in tick
    assert_eq!(compute_next(&Schedule::EveryTicks(500), now, 12_345), 12_845);
    assert_eq!(compute_next(&Schedule::BootPlus(1_000), now, 12_345), 1_000);
    crate::binfo!("svc-check", "compute_next OK");
}

fn check_unitfile() {
    use super::unitfile::{build, Parsed};
    use super::{UnitKind, RestartPolicy, ActivateTarget};
    let doc = super::yaml::parse(
        "name: sshd\ntype: daemon\nexec: /mnt/bin/sshd.wasm\nrestart: on-failure\ntarget: boot\nenabled: true\nafter: [net]\nrequires: [net]\n"
    ).unwrap();
    match build(&doc, Some("sshd.yaml")).expect("build unit") {
        Parsed::U(u) => {
            assert_eq!(u.name, "sshd");
            assert_eq!(u.kind, UnitKind::Daemon);
            assert_eq!(u.restart, RestartPolicy::OnFailure);
            assert_eq!(u.target, ActivateTarget::Boot);
            assert!(u.enabled);
            assert_eq!(u.after, alloc::vec!["net".to_string()]);
            assert_eq!(u.file.as_deref(), Some("sshd.yaml"));
        }
        _ => panic!("expected unit"),
    }
    let tdoc = super::yaml::parse(
        "name: backup\nkind: timer\nunit: backup-job\nschedule: daily 03:00\nenabled: true\n"
    ).unwrap();
    match build(&tdoc, None).expect("build timer") {
        Parsed::T(t) => {
            assert_eq!(t.unit, "backup-job");
            assert_eq!(t.schedule, super::schedule::Schedule::Daily { hour: 3, minute: 0 });
        }
        _ => panic!("expected timer"),
    }
    // difetti: manca name → Err; manca exec → Err; defaults
    assert!(build(&super::yaml::parse("type: daemon\n").unwrap(), None).is_err());
    assert!(build(&super::yaml::parse("name: x\n").unwrap(), None).is_err());
    match build(&super::yaml::parse("name: x\nexec: /bin/x.wasm\n").unwrap(), None).unwrap() {
        Parsed::U(u) => {
            assert_eq!(u.kind, UnitKind::Oneshot);          // default
            assert_eq!(u.restart, RestartPolicy::No);        // default
            assert_eq!(u.target, ActivateTarget::Manual);    // default
            assert!(!u.enabled);                             // default
        }
        _ => panic!("expected unit"),
    }
    crate::binfo!("svc-check", "unitfile OK");
}

fn check_schedule() {
    use super::schedule::{schedule_parse, backoff_ticks, Schedule};
    assert_eq!(schedule_parse("daily 03:00"),    Ok(Schedule::Daily { hour: 3, minute: 0 }));
    assert_eq!(schedule_parse("every 300s"),     Ok(Schedule::EveryTicks(30_000)));
    assert_eq!(schedule_parse("boot+10s"),       Ok(Schedule::BootPlus(1_000)));
    assert_eq!(schedule_parse("hourly :15"),     Ok(Schedule::Hourly { minute: 15 }));
    assert_eq!(schedule_parse("weekly Mon 09:30"), Ok(Schedule::Weekly { dow: 1, hour: 9, minute: 30 }));
    assert!(schedule_parse("daily 25:00").is_err());
    assert!(schedule_parse("garbage").is_err());
    assert_eq!(backoff_ticks(0), 100);   // 1s
    assert_eq!(backoff_ticks(1), 200);   // 2s
    assert_eq!(backoff_ticks(4), 1_600); // 16s
    assert_eq!(backoff_ticks(9), 3_000); // cap 30s
    crate::binfo!("svc-check", "schedule OK");
}

fn check_json() {
    let src = r#"{ "name":"sshd", "type":"daemon", "enabled":true,
                   "after":["net","storage"], "exec":"/mnt/bin/sshd.wasm" }"#;
    let doc = super::json::parse(src).expect("json parse");
    assert_eq!(doc.str_of("name"), Some("sshd"));
    assert_eq!(doc.bool_of("enabled"), Some(true));
    assert_eq!(doc.list_of("after"),
        Some(&["net".to_string(), "storage".to_string()][..]));
    assert!(super::json::parse("{ broken").is_err());
    crate::binfo!("svc-check", "json OK");
}

fn check_yaml() {
    let src = "# commento\nname: sshd\ntype: daemon\nenabled: true\nafter: [net, storage]\n\nexec: /mnt/bin/sshd.wasm\n";
    let doc = super::yaml::parse(src).expect("yaml parse");
    assert_eq!(doc.str_of("name"), Some("sshd"));
    assert_eq!(doc.str_of("type"), Some("daemon"));
    assert_eq!(doc.bool_of("enabled"), Some(true));
    assert_eq!(doc.list_of("after"),
        Some(&["net".to_string(), "storage".to_string()][..]));
    assert_eq!(doc.str_of("exec"), Some("/mnt/bin/sshd.wasm"));
    // riga malformata (niente ':') → errore, non panic
    assert!(super::yaml::parse("solo testo\n").is_err());
    crate::binfo!("svc-check", "yaml OK");
}
