use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct AlbumMeta {
    pub album: String,
    pub artist: String,
    pub count: usize,
    pub pid: String,
}

pub fn load_albums() -> anyhow::Result<Vec<AlbumMeta>> {
    let script = r#"
ObjC.import("Foundation");
const M = Application("Music");
const a = M.tracks.album();
const r = M.tracks.artist();
const p = M.tracks.persistentID();
const map = new Map();
const order = [];
for (let i = 0; i < a.length; i++) {
  const al = a[i], ar = r[i];
  if (!al) continue;
  const k = al + "\u0000" + ar;
  if (!map.has(k)) {
    map.set(k, {album: al, artist: ar, count: 0, pid: p[i]});
    order.push(k);
  }
  map.get(k).count++;
}
JSON.stringify(order.map(k => map.get(k)));
"#;
    let out = Command::new("/usr/bin/osascript")
        .args(["-l", "JavaScript", "-e", script])
        .stderr(Stdio::null())
        .output()?;
    if !out.status.success() {
        anyhow::bail!("osascript failed");
    }
    Ok(serde_json::from_slice(&out.stdout)?)
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
    let artist_name = serde_json::to_string(&album.artist)?;
    let script = format!(
        r#"
const M = Application("Music");
const t = M.tracks.whose({{album: {album_name}}});
const names = t.name(), ids = t.persistentID(), durations = t.duration(), discs = t.discNumber(), numbers = t.trackNumber(), artists = t.artist();
JSON.stringify(names.map((name, i) => ({{name, pid: ids[i], duration: durations[i] || 0, disc: discs[i] || 0, number: numbers[i] || 0, artist: artists[i]}})).filter(track => track.artist === {artist_name}));
"#
    );
    let out = Command::new("/usr/bin/osascript")
        .args(["-l", "JavaScript", "-e", &script])
        .stderr(Stdio::piped())
        .output()?;
    if !out.status.success() {
        anyhow::bail!("{}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let mut tracks: Vec<Track> = serde_json::from_slice(&out.stdout)?;
    tracks.sort_by_key(|track| (track.disc, track.number));
    Ok(tracks)
}

fn art_path(pid: &str) -> PathBuf {
    std::env::temp_dir().join(format!("tera-art-{pid}.jpg"))
}

pub fn fetch_artwork(pid: &str) -> Option<Vec<u8>> {
    let cache = std::env::var_os("HOME").map(|home| {
        PathBuf::from(home)
            .join("Library/Caches/tera/art")
            .join(pid)
    });
    if let Some(bytes) = cache.as_ref().and_then(|p| std::fs::read(p).ok()) {
        if image::load_from_memory(&bytes).is_ok() {
            return Some(bytes);
        }
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
    if let Some(cache) = cache {
        if let Some(dir) = cache.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(cache, &bytes);
    }
    Some(bytes)
}

pub fn decode_cover(bytes: &[u8], size: u32) -> Option<Vec<[u8; 3]>> {
    let img = image::load_from_memory(bytes).ok()?.to_rgb8();
    let img = image::imageops::resize(&img, size, size, image::imageops::FilterType::Triangle);
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
