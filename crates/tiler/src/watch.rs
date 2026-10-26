//! Noticing config file edits.

use std::path::PathBuf;
use std::thread;
use std::time::{Duration, SystemTime};

use crossbeam_channel::Sender;

const POLL_INTERVAL: Duration = Duration::from_secs(1);

fn modified(path: &PathBuf) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// Sends `()` whenever the file's modification time changes (including when it is
/// created or deleted). Polling once a second is plenty for a hand-edited file.
pub fn spawn_config_watcher(path: PathBuf, tx: Sender<()>) {
    thread::Builder::new()
        .name("config-watcher".into())
        .spawn(move || {
            let mut last = modified(&path);
            loop {
                thread::sleep(POLL_INTERVAL);
                let now = modified(&path);
                if now != last {
                    last = now;
                    if tx.send(()).is_err() {
                        return;
                    }
                }
            }
        })
        .expect("failed to spawn the config watcher thread");
}
