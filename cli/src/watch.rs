use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use photosync_core::{is_media, run_client, Config};
use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

pub fn watch(cfg: Config) -> Result<(), Box<dyn std::error::Error>> {
    let (tx, rx) = mpsc::channel();
    let mut watcher: RecommendedWatcher = notify::recommended_watcher(move |res| {
        if let Ok(event) = res {
            let _ = tx.send(event);
        }
    })?;
    watcher.watch(Path::new(&cfg.folder), RecursiveMode::Recursive)?;
    tracing::info!(folder = %cfg.folder.display(), "watching for media changes");

    // Sync before treating events as new work, then drop the index write that sync itself caused.
    let report = run_client(&cfg)?;
    tracing::info!(
        sent = report.sent,
        received = report.received,
        "initial sync finished"
    );
    thread::sleep(Duration::from_millis(300));
    while rx.try_recv().is_ok() {}

    loop {
        let event = rx.recv().map_err(|_| "folder watcher stopped")?;
        if !interesting(&event) {
            continue;
        }
        thread::sleep(Duration::from_secs(1));
        while rx.try_recv().is_ok() {}
        match run_client(&cfg) {
            Ok(report) => {
                tracing::info!(
                    sent = report.sent,
                    received = report.received,
                    "sync finished"
                );
            }
            Err(err) => tracing::error!(error = %err, "sync failed"),
        }
        thread::sleep(Duration::from_millis(200));
        while rx.try_recv().is_ok() {}
    }
}

fn interesting(event: &Event) -> bool {
    event.paths.iter().any(|path| {
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            return false;
        };
        if name.starts_with('.') || name.ends_with(".photosync-partial") {
            return false;
        }
        is_media(path)
    })
}
