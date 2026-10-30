use std::path::Path;

/// Extensions the sync library treats as photos or videos.
/// `jpeg` is accepted because cameras use it interchangeably with `jpg`.
pub fn is_media(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(is_media_name)
}

pub fn is_media_name(name: &str) -> bool {
    let Some((_, ext)) = name.rsplit_once('.') else {
        return false;
    };
    matches!(
        ext.to_ascii_lowercase().as_str(),
        "jpg" | "jpeg" | "png" | "heic" | "mp4" | "mov"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_photo_and_video_extensions() {
        assert!(is_media_name("IMG.JPG"));
        assert!(is_media_name("shot.heic"));
        assert!(is_media_name("clip.MOV"));
        assert!(is_media_name("a.jpeg"));
        assert!(!is_media_name("notes.txt"));
        assert!(!is_media_name("jpg"));
        assert!(!is_media(Path::new("dir")));
    }
}
