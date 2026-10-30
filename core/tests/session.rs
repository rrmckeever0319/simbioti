use photosync_core::{session_as_client, session_as_hub, Config, Role};
use std::fs;
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "photosync-{name}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn peers_exchange_only_missing_media() {
    let psk = "11".repeat(32);
    let root = scratch("session");
    let hub_dir = root.join("hub");
    let client_dir = root.join("client");
    fs::create_dir_all(client_dir.join("album")).unwrap();
    fs::create_dir_all(&hub_dir).unwrap();
    fs::write(hub_dir.join("clip.mov"), b"mov-bytes").unwrap();
    let jpeg = vec![7u8; 70_000];
    fs::write(client_dir.join("album/pic.jpg"), &jpeg).unwrap();
    fs::write(client_dir.join("notes.txt"), b"ignore me").unwrap();

    let once = |hub_dir: &std::path::Path, client_dir: &std::path::Path| {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let hub_cfg = Config::new(
            Role::Hub,
            hub_dir.to_path_buf(),
            Some(addr.to_string()),
            None,
            psk.clone(),
        )
        .unwrap();
        let client_cfg = Config::new(
            Role::Client,
            client_dir.to_path_buf(),
            None,
            Some(addr.to_string()),
            psk.clone(),
        )
        .unwrap();
        let hub = thread::spawn(move || {
            let (sock, _) = listener.accept().unwrap();
            sock.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            sock.set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            session_as_hub(sock, &hub_cfg).unwrap()
        });
        let sock = TcpStream::connect(addr).unwrap();
        sock.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        sock.set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let client_report = session_as_client(sock, &client_cfg).unwrap();
        let hub_report = hub.join().unwrap();
        (client_report, hub_report)
    };

    let (client_report, hub_report) = once(&hub_dir, &client_dir);
    assert_eq!((client_report.sent, client_report.received), (1, 1));
    assert_eq!((hub_report.sent, hub_report.received), (1, 1));
    assert_eq!(fs::read(hub_dir.join("album/pic.jpg")).unwrap(), jpeg);
    assert_eq!(fs::read(client_dir.join("clip.mov")).unwrap(), b"mov-bytes");
    assert!(!hub_dir.join("notes.txt").exists());

    let (client_report, hub_report) = once(&hub_dir, &client_dir);
    assert_eq!((client_report.sent, client_report.received), (0, 0));
    assert_eq!((hub_report.sent, hub_report.received), (0, 0));

    let _ = fs::remove_dir_all(&root);
}
