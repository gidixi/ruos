//! Self-test init-units, gated `boot-checks`. Panica su mismatch (il
//! run-test QEMU fallisce visibilmente); su pass logga una riga per gruppo.
use alloc::string::ToString;

pub fn run() {
    check_yaml();
    check_json();
    check_schedule();
    check_unitfile();
    crate::binfo!("svc-check", "init-units checks OK");
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
