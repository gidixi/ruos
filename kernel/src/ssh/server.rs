//! SSH accept loop + per-session task.
//!
//! Task 6-8 milestone: real `sunset::Runner` event dispatch + ed25519 pubkey
//! auth + PTY-attached shell session.

use spin::Mutex;
use sunset::SignKey;

use crate::executor::delay::Delay;
use alloc::sync::Arc;

use crate::ssh::{authkeys, hostkey, password, sunset_io, CONFIG, SshError};

/// Cached host key + authorized keys + optional password hash, populated by
/// `spawn()` and consumed by the per-session task at accept time.
static SESSION_CTX: Mutex<Option<SessionCtx>> = Mutex::new(None);

struct SessionCtx {
    signing:  ed25519_dalek::SigningKey,
    authkeys: alloc::vec::Vec<[u8; 32]>,
    passwd:   Option<Arc<password::PasswordCheck>>,
}

pub fn spawn() -> Result<(), SshError> {
    crate::ssh::rng_bridge::init_logger();
    let key = hostkey::load_or_generate(CONFIG.host_key_path)?;
    let pub_bytes = key.public();
    crate::binfo!(
        "ssh",
        "host key fingerprint {:02x}{:02x}{:02x}{:02x}…{:02x}{:02x}",
        pub_bytes[0], pub_bytes[1], pub_bytes[2], pub_bytes[3],
        pub_bytes[30], pub_bytes[31],
    );
    let keys   = authkeys::load(CONFIG.authkeys_path)?;
    let passwd = password::load(CONFIG.passwd_path).map(Arc::new);

    *SESSION_CTX.lock() = Some(SessionCtx {
        signing:  key.signing,
        authkeys: keys,
        passwd,
    });

    crate::binfo!("ssh", "listening on 0.0.0.0:{}", CONFIG.port);
    Ok(())
}

pub async fn serve_loop_pub() {
    // Wait until SESSION_CTX is populated (spawn runs from boot before
    // executor::run kicks tasks alive).
    while SESSION_CTX.lock().is_none() { Delay::ticks(1).await; }
    serve_loop().await
}

async fn serve_loop() {
    loop {
        let idx = crate::net::sockets::POOL.alloc_tcp_eth();
        let handle = match crate::net::sockets::POOL.handle(idx) {
            Some(h) => h,
            None    => { Delay::ticks(100).await; continue; }
        };
        if let Err(e) = crate::net::sockets::listen(handle, CONFIG.port) {
            crate::bwarn!("ssh", "listen: {}", e);
            Delay::ticks(100).await;
            continue;
        }
        crate::binfo!("ssh", "accept loop waiting on :{}", CONFIG.port);
        if crate::net::sockets::accept(handle).await.is_err() {
            crate::bwarn!("ssh", "accept failed");
            Delay::ticks(100).await;
            continue;
        }
        crate::binfo!("ssh", "client connected");

        // Snapshot the session ctx for this connection.
        let (signing, authkeys_v, passwd_v) = {
            let g = SESSION_CTX.lock();
            let ctx = g.as_ref().expect("ssh ctx not initialised");
            (ctx.signing.clone(), ctx.authkeys.clone(), ctx.passwd.clone())
        };
        let host_sk = SignKey::Ed25519(signing);

        if let Err(e) = sunset_io::run_session(handle, host_sk, authkeys_v, passwd_v).await {
            crate::bwarn!("ssh", "session: {}", e);
        }
        crate::binfo!("ssh", "client disconnected");
    }
}
