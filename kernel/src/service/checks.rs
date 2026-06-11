//! Self-test init-units, gated `boot-checks`. Panica su mismatch (il
//! run-test QEMU fallisce visibilmente); su pass logga una riga per gruppo.
use alloc::string::ToString;

pub fn run() {
    check_yaml();
    check_json();
    crate::binfo!("svc-check", "init-units checks OK");
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
