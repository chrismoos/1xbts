//! SIGUSR1 stack dumps for diagnosing a stalled real-time loop.

#[cfg(target_os = "macos")]
pub fn install_stack_dump_on_sigusr1() {
    use std::process::Command;
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};

    use log::{error, info, warn};
    use signal_hook::consts::SIGUSR1;
    use signal_hook::iterator::Signals;

    let mut signals = match Signals::new([SIGUSR1]) {
        Ok(s) => s,
        Err(e) => {
            warn!("debug_dump: failed to register SIGUSR1 handler: {e}");
            return;
        }
    };
    let pid = std::process::id();
    info!(
        "debug_dump: SIGUSR1 handler armed (pid={pid}); trigger with `kill -USR1 {pid}` to dump all thread stacks"
    );

    thread::Builder::new()
        .name("debug-stack-dump".into())
        .spawn(move || {
            for _sig in signals.forever() {
                let ts = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let out_path = format!("/tmp/cdma-bts-sample-{pid}-{ts}.txt");
                info!("debug_dump: SIGUSR1 received, sampling pid={pid} → {out_path}");

                let status = Command::new("/usr/bin/sample")
                    .arg(pid.to_string())
                    .arg("1")
                    .arg("-f")
                    .arg(&out_path)
                    .status();

                match status {
                    Ok(s) if s.success() => {
                        info!("debug_dump: stack dump written to {out_path}");
                    }
                    Ok(s) => {
                        error!("debug_dump: /usr/bin/sample exited with status {s}");
                    }
                    Err(e) => {
                        error!("debug_dump: failed to spawn /usr/bin/sample: {e}");
                    }
                }
            }
        })
        .expect("spawn debug-stack-dump thread");
}

// SIGUSR1 stack dumps rely on the macOS `sample` tool, so every other
// platform is a no-op.
#[cfg(not(target_os = "macos"))]
pub fn install_stack_dump_on_sigusr1() {}
