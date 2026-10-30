use crate::error::{Error, Result};
use std::path::{Component, Path, PathBuf};

/// Join `rel` onto `root` and reject absolute paths, `.`, and `..`.
/// The relative path is a `/`-separated string on the wire.
pub fn safe_join(root: &Path, rel: &str) -> Result<PathBuf> {
    if rel.is_empty() || rel.len() > 1024 {
        return Err(Error::Path(format!("rejected length {}", rel.len())));
    }
    let mut out = PathBuf::from(root);
    let mut parts = 0usize;
    for component in Path::new(rel).components() {
        match component {
            Component::Normal(part) => {
                let part = part
                    .to_str()
                    .ok_or_else(|| Error::Path("non-utf8 component".into()))?;
                if part.is_empty() || part == "." || part == ".." {
                    return Err(Error::Path(rel.to_string()));
                }
                out.push(part);
                parts += 1;
            }
            _ => return Err(Error::Path(rel.to_string())),
        }
    }
    if parts == 0 {
        return Err(Error::Path(rel.to_string()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_nested_names_inside_the_folder() {
        let path = safe_join(Path::new("/photos"), "album/pic.jpg").unwrap();
        assert_eq!(path, PathBuf::from("/photos/album/pic.jpg"));
    }

    #[test]
    fn rejects_escape_attempts() {
        assert!(safe_join(Path::new("/photos"), "../secret.jpg").is_err());
        assert!(safe_join(Path::new("/photos"), "/etc/passwd").is_err());
        assert!(safe_join(Path::new("/photos"), "a/../../b.jpg").is_err());
        assert!(safe_join(Path::new("/photos"), "").is_err());
    }
}
