//! Self-test init-units, gated `boot-checks`. Panica su mismatch (il
//! run-test QEMU fallisce visibilmente); su pass logga una riga per gruppo.
use alloc::string::ToString;

pub fn run() {
    check_yaml();
    check_json();
    check_schedule();
    crate::binfo!("svc-check", "init-units checks OK");
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
