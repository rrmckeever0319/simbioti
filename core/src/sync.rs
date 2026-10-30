use crate::config::{Config, Role};
use crate::crypto::SecureConn;
use crate::error::{Error, Result};
use crate::index::{hash_file, mtime_secs, rel_string, FileRecord, Index};
use crate::paths::safe_join;
use crate::proto::{decode, encode, norm_hash, parse_hash, Control, Message, CHUNK_DATA};
use std::collections::HashSet;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub sent: usize,
    pub received: usize,
}

pub fn log_report(report: &SyncReport) {
    if report.sent == 0 && report.received == 0 {
        tracing::info!(sent = 0, received = 0, "nothing to transfer");
    } else {
        tracing::info!(
            sent = report.sent,
            received = report.received,
            "sync complete"
        );
    }
}

pub struct HubServer {
    listener: TcpListener,
    cfg: Config,
}

impl HubServer {
    pub fn bind(cfg: &Config) -> Result<Self> {
        if cfg.role != Role::Hub {
            return Err(Error::Config("hub requires role = \"hub\"".into()));
        }
        cfg.validate()?;
        prepare_folder(&cfg.folder)?;
        let listen = cfg
            .listen
            .clone()
            .ok_or_else(|| Error::Config("hub requires listen".into()))?;
        let listener = TcpListener::bind(&listen)?;
        Ok(Self {
            listener,
            cfg: cfg.clone(),
        })
    }

    pub fn local_addr(&self) -> Result<SocketAddr> {
        Ok(self.listener.local_addr()?)
    }

    /// Accept sessions until `once` is set, then return that session's report.
    pub fn serve(self, once: bool) -> Result<SyncReport> {
        let addr = self.local_addr()?;
        tracing::info!(%addr, "hub listening");
        loop {
            let (sock, peer) = self.listener.accept()?;
            tracing::info!(%peer, "peer connected");
            prepare_tcp(&sock)?;
            let result = session_as_hub(sock, &self.cfg);
            match &result {
                Ok(report) => log_report(report),
                Err(err) => tracing::error!(error = %err, "session failed"),
            }
            if once {
                return result;
            }
        }
    }
}

pub fn run_client(cfg: &Config) -> Result<SyncReport> {
    if cfg.role != Role::Client {
        return Err(Error::Config("client requires role = \"client\"".into()));
    }
    cfg.validate()?;
    prepare_folder(&cfg.folder)?;
    let peer = cfg
        .peer
        .clone()
        .ok_or_else(|| Error::Config("client requires peer".into()))?;
    let addr = resolve(&peer)?;
    tracing::info!(%addr, "connecting");
    let sock = TcpStream::connect_timeout(&addr, Duration::from_secs(10))?;
    prepare_tcp(&sock)?;
    let report = session_as_client(sock, cfg)?;
    log_report(&report);
    Ok(report)
}

pub fn scan_library(folder: &Path) -> Result<usize> {
    prepare_folder(folder)?;
    let (index, stats) = Index::scan(folder)?;
    let count = index.files.len();
    index.save(folder)?;
    tracing::info!(
        hashed = stats.hashed,
        reused = stats.reused,
        files = count,
        "scanned folder"
    );
    Ok(count)
}

pub fn session_as_client<S: Read + Write>(stream: S, cfg: &Config) -> Result<SyncReport> {
    let mut conn = SecureConn::handshake_initiator(stream, &cfg.psk()?)?;
    tracing::info!("handshake complete");
    let (mut index, stats) = scan_for_session(&cfg.folder)?;
    tracing::info!(
        hashed = stats.hashed,
        reused = stats.reused,
        files = index.files.len(),
        "local index ready"
    );

    send_haves(&mut conn, &index)?;
    let remote = read_haves(&mut conn)?;
    tracing::info!(remote_files = remote.len(), "received peer manifest");
    let local_hashes: HashSet<String> = index.files.keys().cloned().collect();
    let remote_hashes: HashSet<String> = remote.keys().cloned().collect();

    let mut report = SyncReport::default();
    let mut to_send: Vec<FileRecord> = index
        .files
        .values()
        .filter(|record| !remote_hashes.contains(&record.hash))
        .cloned()
        .collect();
    to_send.sort_by(|a, b| a.hash.cmp(&b.hash));
    for record in &to_send {
        send_file(&mut conn, &cfg.folder, record)?;
        report.sent += 1;
    }

    let mut needed: Vec<String> = remote
        .keys()
        .filter(|hash| !local_hashes.contains(*hash))
        .cloned()
        .collect();
    needed.sort();
    for hash in &needed {
        send_control(&mut conn, Control::Need { hash: hash.clone() })?;
    }
    send_control(&mut conn, Control::NeedEnd)?;

    loop {
        match recv_message(&mut conn)? {
            Message::Control(Control::FileStart {
                hash,
                size,
                rel_path,
            }) => {
                let written = receive_file(&mut conn, &cfg.folder, &hash, size, &rel_path)?;
                remember(&mut index, &cfg.folder, &written, &norm_hash(&hash)?, size)?;
                report.received += 1;
            }
            Message::Control(Control::Done) => break,
            Message::Control(Control::Error { message }) => {
                return Err(Error::Protocol(format!("peer error: {message}")));
            }
            other => {
                return Err(Error::Protocol(format!(
                    "unexpected message while receiving: {other:?}"
                )))
            }
        }
    }
    send_control(&mut conn, Control::Done)?;
    index.save(&cfg.folder)?;
    Ok(report)
}

pub fn session_as_hub<S: Read + Write>(stream: S, cfg: &Config) -> Result<SyncReport> {
    let mut conn = SecureConn::handshake_responder(stream, &cfg.psk()?)?;
    tracing::info!("handshake complete");
    let (mut index, stats) = scan_for_session(&cfg.folder)?;
    tracing::info!(
        hashed = stats.hashed,
        reused = stats.reused,
        files = index.files.len(),
        "local index ready"
    );

    let remote = read_haves(&mut conn)?;
    tracing::info!(remote_files = remote.len(), "received peer manifest");
    send_haves(&mut conn, &index)?;

    let mut report = SyncReport::default();
    let mut needed = Vec::new();
    loop {
        match recv_message(&mut conn)? {
            Message::Control(Control::FileStart {
                hash,
                size,
                rel_path,
            }) => {
                let written = receive_file(&mut conn, &cfg.folder, &hash, size, &rel_path)?;
                remember(&mut index, &cfg.folder, &written, &norm_hash(&hash)?, size)?;
                report.received += 1;
            }
            Message::Control(Control::Need { hash }) => needed.push(norm_hash(&hash)?),
            Message::Control(Control::NeedEnd) => break,
            Message::Control(Control::Error { message }) => {
                return Err(Error::Protocol(format!("peer error: {message}")));
            }
            other => {
                return Err(Error::Protocol(format!(
                    "unexpected message from client: {other:?}"
                )))
            }
        }
    }

    for hash in needed {
        match index.files.get(&hash).cloned() {
            Some(record) => {
                send_file(&mut conn, &cfg.folder, &record)?;
                report.sent += 1;
            }
            None => tracing::warn!(%hash, "peer asked for a hash that is not local"),
        }
    }
    send_control(&mut conn, Control::Done)?;
    match recv_message(&mut conn)? {
        Message::Control(Control::Done) => {}
        Message::Control(Control::Error { message }) => {
            return Err(Error::Protocol(format!("peer error: {message}")));
        }
        other => {
            return Err(Error::Protocol(format!(
                "expected done from client, got {other:?}"
            )))
        }
    }
    index.save(&cfg.folder)?;
    Ok(report)
}

fn scan_for_session(folder: &Path) -> Result<(Index, crate::index::ScanStats)> {
    let scanned = Index::scan(folder)?;
    scanned.0.save(folder)?;
    Ok(scanned)
}

fn prepare_folder(folder: &Path) -> Result<()> {
    fs::create_dir_all(folder)?;
    if !folder.is_dir() {
        return Err(Error::Config(format!(
            "{} is not a directory",
            folder.display()
        )));
    }
    Ok(())
}

fn prepare_tcp(sock: &TcpStream) -> Result<()> {
    sock.set_nodelay(true)?;
    let timeout = Some(Duration::from_secs(30));
    sock.set_read_timeout(timeout)?;
    sock.set_write_timeout(timeout)?;
    Ok(())
}

fn resolve(addr: &str) -> Result<SocketAddr> {
    addr.to_socket_addrs()?
        .next()
        .ok_or_else(|| Error::Config(format!("cannot resolve {addr}")))
}

fn send_haves<S: Read + Write>(conn: &mut SecureConn<S>, index: &Index) -> Result<()> {
    let mut records: Vec<_> = index.files.values().cloned().collect();
    records.sort_by(|a, b| a.hash.cmp(&b.hash));
    for record in records {
        send_control(
            conn,
            Control::Have {
                hash: record.hash,
                size: record.size,
                rel_path: record.rel_path,
            },
        )?;
    }
    send_control(conn, Control::HaveEnd)
}

fn read_haves<S: Read + Write>(
    conn: &mut SecureConn<S>,
) -> Result<std::collections::BTreeMap<String, FileRecord>> {
    let mut out = std::collections::BTreeMap::new();
    loop {
        match recv_message(conn)? {
            Message::Control(Control::Have {
                hash,
                size,
                rel_path,
            }) => {
                let hash = norm_hash(&hash)?;
                let _ = safe_join(Path::new("."), &rel_path)?;
                out.insert(
                    hash.clone(),
                    FileRecord {
                        hash,
                        size,
                        rel_path,
                        mtime_secs: 0,
                    },
                );
            }
            Message::Control(Control::HaveEnd) => return Ok(out),
            Message::Control(Control::Error { message }) => {
                return Err(Error::Protocol(format!("peer error: {message}")));
            }
            other => {
                return Err(Error::Protocol(format!(
                    "expected have list, got {other:?}"
                )))
            }
        }
    }
}

fn send_file<S: Read + Write>(
    conn: &mut SecureConn<S>,
    folder: &Path,
    record: &FileRecord,
) -> Result<()> {
    let path = safe_join(folder, &record.rel_path)?;
    tracing::info!(path = %record.rel_path, size = record.size, "sending file");
    send_control(
        conn,
        Control::FileStart {
            hash: record.hash.clone(),
            size: record.size,
            rel_path: record.rel_path.clone(),
        },
    )?;
    let hash = parse_hash(&record.hash)?;
    let mut file = File::open(&path)?;
    let mut offset = 0u64;
    let mut buf = vec![0u8; CHUNK_DATA];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        let message = encode(&Message::Chunk {
            hash,
            offset,
            data: buf[..n].to_vec(),
        })?;
        conn.send(&message)?;
        offset += u64::try_from(n).expect("read size fits u64");
    }
    if offset != record.size {
        return Err(Error::Protocol(format!(
            "{} changed during send (index {} bytes, read {offset})",
            record.rel_path, record.size
        )));
    }
    send_control(
        conn,
        Control::FileEnd {
            hash: record.hash.clone(),
        },
    )
}

fn receive_file<S: Read + Write>(
    conn: &mut SecureConn<S>,
    folder: &Path,
    hash_hex: &str,
    size: u64,
    rel_path: &str,
) -> Result<String> {
    let hash_hex = norm_hash(hash_hex)?;
    let hash = parse_hash(&hash_hex)?;
    let dest = safe_join(folder, rel_path)?;
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp_name = format!(".{hash_hex}.photosync-partial");
    let tmp = dest.with_file_name(tmp_name);
    let mut partial = PartialFile::create(&tmp)?;
    let mut hasher = blake3::Hasher::new();
    let mut got = 0u64;

    let final_path = loop {
        match recv_message(conn)? {
            Message::Chunk {
                hash: chunk_hash,
                offset,
                data,
            } => {
                if chunk_hash != hash {
                    return Err(Error::Protocol("chunk hash mismatch".into()));
                }
                if offset != got {
                    return Err(Error::Protocol(format!(
                        "chunk offset {offset}, expected {got}"
                    )));
                }
                partial.write_all(&data)?;
                hasher.update(&data);
                got += u64::try_from(data.len()).expect("chunk fits u64");
                if got > size {
                    return Err(Error::Protocol("file exceeded declared size".into()));
                }
            }
            Message::Control(Control::FileEnd { hash: end_hash }) => {
                if norm_hash(&end_hash)? != hash_hex {
                    return Err(Error::Protocol("file end hash mismatch".into()));
                }
                if got != size {
                    return Err(Error::Protocol(format!(
                        "size mismatch: got {got}, declared {size}"
                    )));
                }
                let actual = hasher.finalize().to_hex().to_string();
                if actual != hash_hex {
                    return Err(Error::Protocol("blake3 mismatch".into()));
                }
                partial.sync()?;
                let placed = place_file(&tmp, &dest, &hash_hex)?;
                partial.committed = true;
                break placed;
            }
            Message::Control(Control::Error { message }) => {
                return Err(Error::Protocol(format!("peer error: {message}")));
            }
            other => {
                return Err(Error::Protocol(format!(
                    "unexpected message during file: {other:?}"
                )))
            }
        }
    };

    let written = rel_string(folder, &final_path)?;
    tracing::info!(path = %written, size, "received file");
    Ok(written)
}

fn place_file(tmp: &Path, dest: &Path, hash_hex: &str) -> Result<PathBuf> {
    if dest.exists() {
        let (existing, _) = hash_file(dest)?;
        if existing == hash_hex {
            let _ = fs::remove_file(tmp);
            return Ok(dest.to_path_buf());
        }
        let conflict = conflict_path(dest, &hash_hex[..8]);
        fs::rename(tmp, &conflict)?;
        tracing::warn!(
            path = %conflict.display(),
            "destination existed with a different hash; stored a new name"
        );
        return Ok(conflict);
    }
    fs::rename(tmp, dest)?;
    Ok(dest.to_path_buf())
}

fn conflict_path(dest: &Path, hash8: &str) -> PathBuf {
    let stem = dest.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let ext = dest.extension().and_then(|s| s.to_str());
    for n in 0..1000 {
        let marker = if n == 0 {
            format!("photosync-{hash8}")
        } else {
            format!("photosync-{hash8}-{n}")
        };
        let name = match ext {
            Some(ext) => format!("{stem}.{marker}.{ext}"),
            None => format!("{stem}.{marker}"),
        };
        let candidate = dest.with_file_name(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    dest.with_file_name(format!("{stem}.photosync-{hash8}-overflow"))
}

fn remember(index: &mut Index, folder: &Path, rel_path: &str, hash: &str, size: u64) -> Result<()> {
    let path = safe_join(folder, rel_path)?;
    index.files.insert(
        hash.to_string(),
        FileRecord {
            hash: hash.to_string(),
            size,
            rel_path: rel_path.to_string(),
            mtime_secs: mtime_secs(&path),
        },
    );
    Ok(())
}

fn send_control<S: Read + Write>(conn: &mut SecureConn<S>, control: Control) -> Result<()> {
    let bytes = encode(&Message::Control(control))?;
    conn.send(&bytes)
}

fn recv_message<S: Read + Write>(conn: &mut SecureConn<S>) -> Result<Message> {
    let bytes = conn.recv()?;
    decode(&bytes)
}

struct PartialFile {
    path: PathBuf,
    file: Option<File>,
    committed: bool,
}

impl PartialFile {
    fn create(path: &Path) -> Result<Self> {
        Ok(Self {
            file: Some(File::create(path)?),
            path: path.to_path_buf(),
            committed: false,
        })
    }

    fn write_all(&mut self, data: &[u8]) -> Result<()> {
        self.file
            .as_mut()
            .ok_or_else(|| Error::Protocol("partial file already closed".into()))?
            .write_all(data)?;
        Ok(())
    }

    fn sync(&mut self) -> Result<()> {
        if let Some(file) = self.file.take() {
            file.sync_all()?;
        }
        Ok(())
    }
}

impl Drop for PartialFile {
    fn drop(&mut self) {
        self.file.take();
        if !self.committed {
            let _ = fs::remove_file(&self.path);
        }
    }
}
