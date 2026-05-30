//! SSH accept loop + per-session task.
//!
//! Task 5 milestone: socket bind + listen + serve_one stub (byte echo).
//! Full sunset Runner integration + auth + channel dispatch land in
//! Tasks 6-8.

use crate::executor::delay::Delay;
use crate::ssh::{authkeys, hostkey, sunset_io, CONFIG, SshError};

pub fn spawn() -> Result<(), SshError> {
    let key = hostkey::load_or_generate(CONFIG.host_key_path)?;
    let pub_bytes = key.public();
    crate::binfo!(
        "ssh",
        "host key fingerprint {:02x}{:02x}{:02x}{:02x}…{:02x}{:02x}",
        pub_bytes[0], pub_bytes[1], pub_bytes[2], pub_bytes[3],
        pub_bytes[30], pub_bytes[31],
    );
    let _keys = authkeys::load(CONFIG.authkeys_path)?;

    // The accept loop runs as a dedicated embassy task spawned in
    // `crate::executor::run`. Here we just persist any state the task
    // needs (none yet; key/keys are reloaded inside the task in Task 6).
    crate::binfo!("ssh", "listening on 0.0.0.0:{} (task pending start)", CONFIG.port);
    Ok(())
}

pub async fn serve_loop_pub() { serve_loop().await }

async fn serve_loop() {
    loop {
        // Fresh socket per accept (smoltcp's accept transitions the
        // listening socket into Established).
        let idx = crate::net::sockets::POOL.alloc_tcp();
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
        if let Err(e) = sunset_io::run_session(handle).await {
            crate::bwarn!("ssh", "session: {}", e);
        }
        crate::binfo!("ssh", "client disconnected");
    }
}
