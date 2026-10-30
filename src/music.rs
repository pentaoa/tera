use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct AlbumMeta {
    pub album: String,
    pub artist: String,
    pub count: usize,
    pub pid: String,
    pub track_ids: Vec<String>,
    pub track_artists: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct AlbumTrack {
    album: String,
    artist: String,
    album_artist: String,
    compilation: bool,
    pid: String,
}

fn group_albums(tracks: Vec<AlbumTrack>) -> Vec<AlbumMeta> {
    let mut albums: Vec<AlbumMeta> = Vec::new();
    let mut positions = HashMap::new();
    for track in tracks {
        if track.album.is_empty() {
            continue;
        }
        let artist = if !track.album_artist.trim().is_empty() {
            track.album_artist.clone()
        } else if track.compilation {
            "Various Artists".to_string()
        } else {
            track.artist.clone()
        };
        let key = (track.album.clone(), artist.clone());
        let index = *positions.entry(key).or_insert_with(|| {
            albums.push(AlbumMeta {
                album: track.album.clone(),
                artist,
                count: 0,
                pid: track.pid.clone(),
                track_ids: Vec::new(),
                track_artists: Vec::new(),
            });
            albums.len() - 1
        });
        let album = &mut albums[index];
        album.count += 1;
        album.track_ids.push(track.pid);
        if !album.track_artists.contains(&track.artist) {
            album.track_artists.push(track.artist);
        }
    }
    albums
}

pub fn load_albums() -> anyhow::Result<Vec<AlbumMeta>> {
    let script = r#"
ObjC.import("Foundation");
const M = Application("Music");
const a = M.tracks.album();
const r = M.tracks.artist();
const aa = M.tracks.albumArtist();
const c = M.tracks.compilation();
const p = M.tracks.persistentID();
JSON.stringify(a.map((album, i) => ({
  album: album || "", artist: r[i] || "", album_artist: aa[i] || "",
  compilation: !!c[i], pid: p[i] || ""
})));
"#;
    let out = Command::new("/usr/bin/osascript")
        .args(["-l", "JavaScript", "-e", script])
        .stderr(Stdio::null())
        .output()?;
    if !out.status.success() {
        anyhow::bail!("osascript failed");
    }
    let tracks: Vec<AlbumTrack> = serde_json::from_slice(&out.stdout)?;
    Ok(group_albums(tracks))
}

#[derive(Debug, Clone, Deserialize)]
pub struct Track {
    pub name: String,
    pub pid: String,
    pub duration: f64,
    pub disc: u32,
    pub number: u32,
}

pub fn load_tracks(album: &AlbumMeta) -> anyhow::Result<Vec<Track>> {
    let album_name = serde_json::to_string(&album.album)?;
    let script = format!(
        r#"
const M = Application("Music");
const t = M.tracks.whose({{album: {album_name}}});
const names = t.name(), ids = t.persistentID(), durations = t.duration(), discs = t.discNumber(), numbers = t.trackNumber();
JSON.stringify(names.map((name, i) => ({{name, pid: ids[i], duration: durations[i] || 0, disc: discs[i] || 0, number: numbers[i] || 0}})));
"#
    );
    let out = Command::new("/usr/bin/osascript")
        .args(["-l", "JavaScript", "-e", &script])
        .stderr(Stdio::piped())
        .output()?;
    if !out.status.success() {
        anyhow::bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let tracks: Vec<Track> = serde_json::from_slice(&out.stdout)?;
    Ok(select_album_tracks(album, tracks))
}

fn select_album_tracks(album: &AlbumMeta, mut tracks: Vec<Track>) -> Vec<Track> {
    let ids: HashSet<&str> = album.track_ids.iter().map(String::as_str).collect();
    tracks.retain(|track| ids.contains(track.pid.as_str()));
    tracks.sort_by_key(|track| (track.disc, track.number));
    tracks
}

fn art_path(pid: &str) -> PathBuf {
    std::env::temp_dir().join(format!("tera-art-{pid}.jpg"))
}

pub fn load_cover(pid: &str, size: u32) -> Option<Vec<[u8; 3]>> {
    let cache = std::env::var_os("HOME").map(|home| {
        PathBuf::from(home)
            .join("Library/Caches/tera/art")
            .join(pid)
    });
    if let Some(pixels) = cache
        .as_ref()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|bytes| decode_cover(&bytes, size))
    {
        return Some(pixels);
    }
    let path = art_path(pid);
    let _ = std::fs::remove_file(&path);
    let script = format!(
        r#"
tell application "Music"
    set t to first track whose persistent ID is "{pid}"
    set d to data of artwork 1 of t
end tell
set f to open for access POSIX file "{p}" with write permission
set eof f to 0
write d to f
close access f
"#,
        pid = pid,
        p = path.display()
    );
    let ok = Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(script)
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        return None;
    }
    let bytes = std::fs::read(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    if bytes.len() < 4 {
        return None;
    }
    let pixels = decode_cover(&bytes, size)?;
    if let Some(cache) = cache {
        if let Some(dir) = cache.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(cache, &bytes);
    }
    Some(pixels)
}

pub fn decode_cover(bytes: &[u8], size: u32) -> Option<Vec<[u8; 3]>> {
    let img = image::load_from_memory(bytes).ok()?;
    // Fill a square from the centre instead of stretching rectangular artwork.
    let img = img
        .resize_to_fill(size, size, image::imageops::FilterType::Triangle)
        .to_rgb8();
    let mut out = Vec::with_capacity((size * size) as usize);
    for p in img.pixels() {
        out.push([p[0], p[1], p[2]]);
    }
    Some(out)
}

pub fn play_pause() {
    let _ = Command::new("/usr/bin/osascript")
        .args(["-e", "tell application \"Music\" to playpause"])
        .stderr(Stdio::null())
        .status();
}

pub fn play_track(pid: &str) {
    if pid.is_empty() {
        return;
    }
    let script = format!(
        "tell application \"Music\" to play (first track whose persistent ID is \"{pid}\")"
    );
    let _ = Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(script)
        .stderr(Stdio::null())
        .status();
}

pub fn is_playing() -> bool {
    let out = Command::new("/usr/bin/osascript")
        .args([
            "-e",
            "tell application \"Music\" to get player state as string",
        ])
        .stderr(Stdio::null())
        .output();
    match out {
        Ok(o) => String::from_utf8_lossy(&o.stdout).trim() == "playing",
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, ImageEncoder, Rgb};

    fn album_track(
        album: &str,
        artist: &str,
        album_artist: &str,
        compilation: bool,
        pid: &str,
    ) -> AlbumTrack {
        AlbumTrack {
            album: album.into(),
            artist: artist.into(),
            album_artist: album_artist.into(),
            compilation,
            pid: pid.into(),
        }
    }

    #[test]
    fn groups_by_album_artist_and_keeps_same_named_albums_distinct() {
        let groups = group_albums(vec![
            album_track("Soundtrack", "Singer A", "Composer", false, "1"),
            album_track("Soundtrack", "Singer B", "Composer", false, "2"),
            album_track("Soundtrack", "Singer A", "Another Artist", false, "3"),
            album_track("Compilation", "Singer A", "", true, "4"),
            album_track("Compilation", "Singer B", "", true, "5"),
            album_track("Solo", "Singer A", "", false, "6"),
        ]);
        assert_eq!(groups.len(), 4);
        assert_eq!(groups[0].artist, "Composer");
        assert_eq!(groups[0].track_ids, ["1", "2"]);
        assert_eq!(groups[0].track_artists, ["Singer A", "Singer B"]);
        assert_eq!(groups[1].track_ids, ["3"]);
        assert_eq!(groups[2].artist, "Various Artists");
        assert_eq!(groups[2].track_ids, ["4", "5"]);
        assert_eq!(groups[3].artist, "Singer A");
    }

    #[test]
    fn song_list_uses_the_group_ids_and_sorts_discs() {
        let album = group_albums(vec![
            album_track("Shared", "A", "A", false, "1"),
            album_track("Shared", "A", "A", false, "2"),
        ])
        .remove(0);
        let track = |pid: &str, disc, number| Track {
            name: pid.into(),
            pid: pid.into(),
            duration: 0.0,
            disc,
            number,
        };
        let tracks = select_album_tracks(
            &album,
            vec![track("2", 2, 1), track("other", 1, 1), track("1", 1, 2)],
        );
        assert_eq!(
            tracks.iter().map(|t| t.pid.as_str()).collect::<Vec<_>>(),
            ["1", "2"]
        );
    }

    #[test]
    #[ignore = "requires an accessible Music.app library with multi-artist albums"]
    fn live_library_groups_and_queries_the_same_tracks() {
        let albums = load_albums().expect("load Music library");
        let album = albums
            .iter()
            .filter(|album| album.track_artists.len() > 1 && album.count > 1)
            .max_by_key(|album| album.track_artists.len())
            .expect("library needs a multi-artist album");
        let tracks = load_tracks(album).expect("load album tracks");
        assert_eq!(album.count, tracks.len());
        let expected: HashSet<_> = album.track_ids.iter().collect();
        let actual: HashSet<_> = tracks.iter().map(|track| &track.pid).collect();
        assert_eq!(expected, actual);
    }

    #[test]
    fn rectangular_cover_is_center_cropped_without_stretching() {
        let source = ImageBuffer::from_fn(8, 4, |x, _| Rgb([x as u8 * 30, 0, 0]));
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(source.as_raw(), 8, 4, image::ExtendedColorType::Rgb8)
            .unwrap();
        let pixels = decode_cover(&png, 4).unwrap();
        assert_eq!(pixels.len(), 16);
        assert_eq!(pixels[0][0], 60);
        assert_eq!(pixels[3][0], 150);
        assert_eq!(pixels[12][0], 60);
    }
}
