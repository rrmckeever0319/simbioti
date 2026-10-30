use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_photosync")
}

fn scratch() -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "photosync-proc-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_config(
    path: &Path,
    role: &str,
    folder: &Path,
    listen: Option<&str>,
    peer: Option<&str>,
    psk: &str,
) {
    let mut text = format!("role = \"{role}\"\nfolder = \"{}\"\n", folder.display());
    if let Some(listen) = listen {
        text.push_str(&format!("listen = \"{listen}\"\n"));
    }
    if let Some(peer) = peer {
        text.push_str(&format!("peer = \"{peer}\"\n"));
    }
    text.push_str(&format!("psk_hex = \"{psk}\"\n"));
    fs::write(path, text).unwrap();
}

struct Hub {
    child: std::process::Child,
    stderr: StringHandle,
}

struct StringHandle {
    handle: Option<thread::JoinHandle<String>>,
}

impl Hub {
    fn spawn(config: &Path) -> Self {
        let mut child = Command::new(bin())
            .args(["hub", "--config"])
            .arg(config)
            .env("RUST_LOG", "info")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let stderr = child.stderr.take().unwrap();
        let handle = thread::spawn(move || {
            let mut out = String::new();
            let mut reader = BufReader::new(stderr);
            let mut line = String::new();
            while reader.read_line(&mut line).unwrap_or(0) > 0 {
                out.push_str(&line);
                line.clear();
            }
            out
        });
        Self {
            child,
            stderr: StringHandle {
                handle: Some(handle),
            },
        }
    }

    fn listen_addr(&mut self) -> String {
        let stdout = self.child.stdout.take().unwrap();
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        let n = reader.read_line(&mut line).unwrap();
        assert!(n > 0, "hub exited before printing its listen address");
        let addr = line
            .trim()
            .strip_prefix("PHOTOSYNC_LISTEN ")
            .unwrap_or_else(|| panic!("unexpected hub stdout: {line}"))
            .to_string();
        self.child.stdout = Some(reader.into_inner());
        addr
    }

    fn stop(mut self) -> String {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.stderr.handle.take().unwrap().join().unwrap()
    }
}

fn run_client(config: &Path) -> std::process::Output {
    let output = Command::new(bin())
        .args(["client", "--config"])
        .arg(config)
        .env("RUST_LOG", "info")
        .output()
        .unwrap();
    if !output.status.success() {
        panic!(
            "client failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    output
}

#[test]
fn two_processes_sync_missing_files_once() {
    let root = scratch();
    let hub_dir = root.join("hub");
    let client_dir = root.join("client");
    fs::create_dir_all(client_dir.join("album")).unwrap();
    fs::create_dir_all(&hub_dir).unwrap();
    fs::write(hub_dir.join("clip.mov"), b"mov-bytes").unwrap();
    let jpeg = vec![7u8; 70_000];
    fs::write(client_dir.join("album/pic.jpg"), &jpeg).unwrap();
    fs::write(client_dir.join("notes.txt"), b"ignore me").unwrap();

    let key = Command::new(bin()).arg("keygen").output().unwrap();
    assert!(key.status.success());
    let psk = String::from_utf8(key.stdout).unwrap();
    let psk = psk.trim();
    assert_eq!(psk.len(), 64);

    let hub_cfg = root.join("hub.toml");
    let client_cfg = root.join("client.toml");
    write_config(&hub_cfg, "hub", &hub_dir, Some("127.0.0.1:0"), None, psk);
    // Peer is filled in after the hub prints the real port.
    write_config(
        &client_cfg,
        "client",
        &client_dir,
        None,
        Some("127.0.0.1:9"),
        psk,
    );

    let mut hub = Hub::spawn(&hub_cfg);
    let addr = hub.listen_addr();
    write_config(&client_cfg, "client", &client_dir, None, Some(&addr), psk);

    let first = run_client(&client_cfg);
    let first_out = String::from_utf8_lossy(&first.stdout);
    assert!(
        first_out.contains("sent 1 received 1"),
        "first sync stdout: {first_out}"
    );
    assert_eq!(fs::read(hub_dir.join("album/pic.jpg")).unwrap(), jpeg);
    assert_eq!(fs::read(client_dir.join("clip.mov")).unwrap(), b"mov-bytes");
    assert!(!hub_dir.join("notes.txt").exists());

    thread::sleep(Duration::from_millis(200));
    let second = run_client(&client_cfg);
    let second_out = String::from_utf8_lossy(&second.stdout);
    let second_err = String::from_utf8_lossy(&second.stderr);
    assert!(
        second_out.contains("sent 0 received 0"),
        "second stdout: {second_out}\nstderr: {second_err}"
    );
    assert!(
        second_err.contains("nothing to transfer"),
        "second stderr: {second_err}"
    );

    let hub_log = hub.stop();
    assert!(hub_log.contains("handshake complete"), "hub log: {hub_log}");
    let _ = fs::remove_dir_all(&root);
}
