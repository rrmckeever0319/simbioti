use crate::error::{Error, Result};
use crate::media::is_media_name;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

pub const INDEX_NAME: &str = ".photosync-index.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRecord {
    pub hash: String,
    pub size: u64,
    pub rel_path: String,
    pub mtime_secs: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Index {
    pub files: BTreeMap<String, FileRecord>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanStats {
    pub hashed: usize,
    pub reused: usize,
}

impl Index {
    pub fn index_path(folder: &Path) -> PathBuf {
        folder.join(INDEX_NAME)
    }

    pub fn load(folder: &Path) -> Result<Self> {
        let path = Self::index_path(folder);
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = fs::read_to_string(&path)?;
        serde_json::from_str(&text).map_err(|err| Error::Index(err.to_string()))
    }

    pub fn save(&self, folder: &Path) -> Result<()> {
        let path = Self::index_path(folder);
        let tmp = folder.join(".photosync-index.json.photosync-partial");
        let mut body =
            serde_json::to_vec_pretty(self).map_err(|err| Error::Index(err.to_string()))?;
        body.push(b'\n');
        {
            let mut file = File::create(&tmp)?;
            std::io::Write::write_all(&mut file, &body)?;
            file.sync_all()?;
        }
        fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// Walk `folder` and build an index keyed by BLAKE3 hex.
    /// A file is rehashed only when its size or mtime differs from the saved index.
    /// Identical content is stored once; the last path in sorted order is kept.
    pub fn scan(folder: &Path) -> Result<(Self, ScanStats)> {
        let previous = Self::load(folder)?;
        let mut by_path = HashMap::new();
        for record in previous.files.values() {
            by_path.insert(record.rel_path.clone(), record.clone());
        }
        let mut files = BTreeMap::new();
        let mut stats = ScanStats::default();
        visit(folder, folder, &by_path, &mut files, &mut stats)?;
        Ok((Self { files }, stats))
    }
}

pub fn hash_file(path: &Path) -> Result<(String, u64)> {
    let mut file = File::open(path)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = [0u8; 64 * 1024];
    let mut size = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        size += u64::try_from(n).expect("read size fits u64");
    }
    Ok((hasher.finalize().to_hex().to_string(), size))
}

pub(crate) fn mtime_secs(path: &Path) -> i64 {
    fs::metadata(path)
        .ok()
        .and_then(|meta| meta.modified().ok())
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(-1)
}

pub(crate) fn rel_string(root: &Path, path: &Path) -> Result<String> {
    let rel = path
        .strip_prefix(root)
        .map_err(|_| Error::Path(format!("not under {}", root.display())))?;
    let mut out = String::new();
    for component in rel.components() {
        match component {
            Component::Normal(part) => {
                if !out.is_empty() {
                    out.push('/');
                }
                let part = part
                    .to_str()
                    .ok_or_else(|| Error::Path("non-utf8 path".into()))?;
                out.push_str(part);
            }
            _ => {
                return Err(Error::Path(format!(
                    "unexpected component in {}",
                    path.display()
                )))
            }
        }
    }
    if out.is_empty() {
        return Err(Error::Path(path.display().to_string()));
    }
    Ok(out)
}

fn visit(
    root: &Path,
    dir: &Path,
    previous: &HashMap<String, FileRecord>,
    files: &mut BTreeMap<String, FileRecord>,
    stats: &mut ScanStats,
) -> Result<()> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(dir)? {
        entries.push(entry?);
    }
    entries.sort_by_key(|entry| entry.file_name());

    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            visit(root, &path, previous, files, stats)?;
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let name = entry.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| Error::Path(format!("non-utf8 file name in {}", dir.display())))?;
        if name.starts_with('.') || name.ends_with(".photosync-partial") || !is_media_name(name) {
            continue;
        }

        let rel_path = rel_string(root, &path)?;
        let meta = entry.metadata()?;
        let size = meta.len();
        let mtime = meta
            .modified()
            .ok()
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .map(|duration| i64::try_from(duration.as_secs()).unwrap_or(i64::MAX))
            .unwrap_or(-1);

        if let Some(old) = previous.get(&rel_path) {
            if old.size == size && old.mtime_secs == mtime && old.mtime_secs >= 0 {
                files.insert(old.hash.clone(), old.clone());
                stats.reused += 1;
                continue;
            }
        }

        let (hash, hashed_size) = hash_file(&path)?;
        files.insert(
            hash.clone(),
            FileRecord {
                hash,
                size: hashed_size,
                rel_path,
                mtime_secs: mtime,
            },
        );
        stats.hashed += 1;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "photosync-index-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn scan_ignores_text_and_reuses_unchanged_files() {
        let dir = scratch();
        fs::write(dir.join("notes.txt"), b"hello").unwrap();
        fs::write(dir.join(".hidden.jpg"), b"secret").unwrap();
        fs::write(dir.join("pic.jpg"), b"jpeg-bytes").unwrap();

        let (index, stats) = Index::scan(&dir).unwrap();
        assert_eq!(stats.hashed, 1);
        assert_eq!(stats.reused, 0);
        assert_eq!(index.files.len(), 1);
        let record = index.files.values().next().unwrap();
        assert_eq!(record.rel_path, "pic.jpg");
        index.save(&dir).unwrap();

        let (again, stats) = Index::scan(&dir).unwrap();
        assert_eq!(stats.hashed, 0);
        assert_eq!(stats.reused, 1);
        assert_eq!(again.files, index.files);

        let _ = fs::remove_dir_all(&dir);
    }
}
