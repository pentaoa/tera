mod click;
mod haptic;
mod kitty;
mod music;
mod session;
mod settings;
mod sprite;

use std::collections::HashSet;
use std::io::{self, BufWriter, Write};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, MouseEventKind,
};
use crossterm::terminal::{BeginSynchronizedUpdate, Clear, ClearType, EndSynchronizedUpdate};
use crossterm::{cursor, execute, queue, terminal};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use unicode_width::UnicodeWidthStr;

use music::{AlbumMeta, Track};

const ART_SIZE: u32 = 512;

struct Cover {
    px: Arc<Vec<[u8; 3]>>,
    w: usize,
    h: usize,
}

enum PlaybackCommand {
    Toggle,
    Play(String),
}

enum Msg {
    Albums(Vec<AlbumMeta>),
    Art {
        idx: usize,
        px: Vec<[u8; 3]>,
        png: Option<Vec<u8>>,
        sprites: Option<Vec<Vec<u8>>>,
    },
    Sprite {
        idx: usize,
        bin: usize,
        png: Vec<u8>,
    },
    Skipped(usize),
    SpriteSkipped(usize, usize),
    Error(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ViewMode {
    Flow,
    Grid,
}

struct WallChange {
    slot: usize,
    next: usize,
    started: Instant,
    effect: u8,
    swap_slot: Option<usize>,
}

#[derive(Clone, PartialEq, Eq)]
struct ActivePlacement {
    image_id: u32,
    z: i32,
    command: String,
}

struct App {
    albums: Vec<AlbumMeta>,
    covers: Vec<Option<Cover>>,
    fallback: Vec<Cover>,
    selected: usize,
    pos: f32,
    fps: f64,
    playing: bool,
    status: String,
    tx_req: Option<Sender<usize>>,
    wanted: Arc<Mutex<HashSet<usize>>>,
    tx_sprites: Option<Sender<(usize, usize, Arc<Vec<[u8; 3]>>, u32)>>,
    sprites_requested: HashSet<(usize, usize)>,
    sprite_wanted: Arc<Mutex<HashSet<(usize, usize)>>>,
    requested: HashSet<usize>,
    rx: Option<Receiver<Msg>>,
    pngs: Vec<Option<Vec<u8>>>,
    sprites: Vec<Option<Vec<Vec<u8>>>>,
    uploaded: HashSet<u32>,
    active_image: std::collections::HashMap<usize, ActivePlacement>,
    pending_upload: Vec<usize>,
    pending_deletions: Vec<(usize, u32)>,
    kitty: bool,
    forced_blocks: bool,
    mode: ViewMode,
    wall: Vec<usize>,
    wall_slot: usize,
    settings: settings::Settings,
    session: session::Session,
    settings_open: bool,
    settings_row: usize,
    scroll_accum: f32,
    scroll_dir: i8,
    scroll_last_event: Instant,
    scroll_last_move: Instant,
    wall_change: Option<WallChange>,
    wall_pending: Option<(usize, usize, Instant)>,
    wall_next: Instant,
    wall_random: u64,
    detail_open: bool,
    flip: f32,
    tracks: Vec<Track>,
    track_selected: usize,
    track_status: String,
    track_rx: Option<Receiver<(usize, Result<Vec<Track>, String>)>>,
}

impl App {
    fn synthetic() -> Self {
        let names = [
            "Blue Hour",
            "Paper Moon",
            "Analog Dreams",
            "Night Drive",
            "Velvet Static",
            "Glass Garden",
            "Neon Tide",
            "Slow Motion",
            "Harbor Lights",
            "Amber Room",
            "Quiet Riot",
            "Silver Lining",
        ];
        let albums = names
            .iter()
            .map(|n| AlbumMeta {
                album: n.to_string(),
                artist: "tera".into(),
                count: 10,
                pid: String::new(),
                track_ids: Vec::new(),
                track_artists: vec!["tera".into()],
            })
            .collect::<Vec<_>>();
        let n = albums.len();
        let fallback = (0..n).map(fallback_cover).collect();
        Self {
            covers: (0..n).map(|_| None).collect(),
            fallback,
            albums,
            selected: 0,
            pos: 0.0,
            fps: 0.0,
            playing: false,
            status: String::new(),
            tx_req: None,
            wanted: Arc::new(Mutex::new(HashSet::new())),
            tx_sprites: None,
            sprites_requested: HashSet::new(),
            sprite_wanted: Arc::new(Mutex::new(HashSet::new())),
            requested: HashSet::new(),
            rx: None,
            pngs: (0..n).map(|_| None).collect(),
            sprites: (0..n).map(|_| None).collect(),
            uploaded: HashSet::new(),
            active_image: std::collections::HashMap::new(),
            pending_upload: Vec::new(),
            pending_deletions: Vec::new(),
            kitty: kitty::supported(),
            forced_blocks: false,
            mode: ViewMode::Flow,
            wall: Vec::new(),
            wall_slot: 0,
            settings: settings::Settings::default(),
            session: session::Session::default(),
            settings_open: false,
            settings_row: 0,
            scroll_accum: 0.0,
            scroll_dir: 0,
            scroll_last_event: Instant::now(),
            scroll_last_move: Instant::now() - Duration::from_secs(1),
            wall_change: None,
            wall_pending: None,
            wall_next: Instant::now() + Duration::from_secs(3),
            wall_random: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64
                | 1,
            detail_open: false,
            flip: 0.0,
            tracks: Vec::new(),
            track_selected: 0,
            track_status: String::new(),
            track_rx: None,
        }
    }

    fn with_music() -> Self {
        let mut app = Self::synthetic();
        app.albums.clear();
        app.covers.clear();
        app.fallback.clear();
        app.pngs.clear();
        app.sprites.clear();
        app.status = "loading music library…".into();
        let (tx_msg, rx_msg) = mpsc::channel::<Msg>();
        let (tx_req, rx_req) = mpsc::channel::<usize>();
        app.tx_req = Some(tx_req);
        app.tx_sprites = Some(spawn_sprite_worker(
            tx_msg.clone(),
            Arc::clone(&app.sprite_wanted),
        ));
        let wanted = Arc::clone(&app.wanted);
        thread::spawn(move || loader(tx_msg, rx_req, wanted));
        app.rx = Some(rx_msg);
        app
    }

    fn restore_selection(&mut self, explicit_start: Option<usize>) {
        if self.albums.is_empty() {
            return;
        }
        let index = explicit_start
            .map(|i| i.min(self.albums.len() - 1))
            .or_else(|| {
                if self.mode == ViewMode::Grid {
                    return None;
                }
                let saved_album = self.session.album.as_deref();
                let saved_artist = self.session.artist.as_deref();
                self.albums
                    .iter()
                    .position(|album| {
                        Some(album.album.as_str()) == saved_album
                            && Some(album.artist.as_str()) == saved_artist
                    })
                    .or_else(|| {
                        self.albums.iter().position(|album| {
                            Some(album.album.as_str()) == saved_album
                                && saved_artist.is_some_and(|artist| {
                                    album.track_artists.iter().any(|name| name == artist)
                                })
                        })
                    })
            })
            .unwrap_or(0);
        self.selected = index;
        self.pos = index as f32;
    }

    fn move_flow(&mut self, direction: i8) {
        // Don't let key repeat build a long queue beyond the visible cover.
        if (self.selected as f32 - self.pos) * direction as f32 >= 1.0 {
            return;
        }
        let next = if direction < 0 {
            self.selected.saturating_sub(1)
        } else {
            (self.selected + 1).min(self.albums.len().saturating_sub(1))
        };
        if next != self.selected {
            self.selected = next;
            self.request_around();
        }
    }

    fn record_session(&mut self) {
        self.session.grid = self.mode == ViewMode::Grid;
        if self.mode == ViewMode::Flow {
            if let Some(album) = self.albums.get(self.selected) {
                self.session.album = Some(album.album.clone());
                self.session.artist = Some(album.artist.clone());
            }
        }
    }

    fn open_detail(&mut self) {
        let Some(album) = self.albums.get(self.selected).cloned() else {
            return;
        };
        self.detail_open = true;
        if self.mode == ViewMode::Grid || !self.settings.album_flip {
            self.flip = 1.0;
        }
        self.track_selected = 0;
        self.tracks.clear();
        if album.pid.is_empty() {
            self.tracks = (1..=album.count)
                .map(|number| Track {
                    name: format!("track {number:02}"),
                    pid: String::new(),
                    duration: 180.0,
                    disc: 1,
                    number: number as u32,
                })
                .collect();
            self.track_status.clear();
        } else {
            self.track_status = "loading tracks…".into();
            let (tx, rx) = mpsc::channel();
            let selected = self.selected;
            thread::spawn(move || {
                let _ = tx.send((
                    selected,
                    music::load_tracks(&album).map_err(|e| e.to_string()),
                ));
            });
            self.track_rx = Some(rx);
        }
    }

    fn poll_tracks(&mut self) -> bool {
        if let Some(rx) = &self.track_rx {
            if let Ok((idx, result)) = rx.try_recv() {
                if idx == self.selected {
                    match result {
                        Ok(tracks) => {
                            self.tracks = tracks;
                            self.track_status = if self.tracks.is_empty() {
                                "no tracks found".into()
                            } else {
                                String::new()
                            };
                        }
                        Err(error) => self.track_status = format!("could not load tracks: {error}"),
                    }
                }
                self.track_rx = None;
                return true;
            }
        }
        false
    }

    fn random(&mut self, limit: usize) -> usize {
        self.wall_random ^= self.wall_random << 13;
        self.wall_random ^= self.wall_random >> 7;
        self.wall_random ^= self.wall_random << 17;
        (self.wall_random as usize) % limit
    }

    fn sync_wall(&mut self, area: Rect) {
        if self.albums.is_empty() {
            return;
        }
        let (cols, rows, _, _, _) = grid_metrics(area);
        let count = (cols * rows).min(self.albums.len());
        if self.wall.len() != count {
            self.wall.clear();
            let start = self.selected.saturating_sub(self.selected % rows);
            for offset in 0..count {
                self.wall.push((start + offset) % self.albums.len());
            }
            self.wall_change = None;
            self.wall_pending = None;
        }
        if let Some(slot) = self.wall.iter().position(|&idx| idx == self.selected) {
            self.wall_slot = slot;
        } else {
            self.wall_slot = 0;
            self.wall[0] = self.selected;
        }
    }

    fn move_wall_selection(&mut self, delta: isize) {
        if self.wall.is_empty() {
            return;
        }
        let slot = self.wall_slot as isize + delta;
        if slot >= 0 && (slot as usize) < self.wall.len() {
            if self.wall_change.as_ref().is_some_and(|change| {
                change.slot == slot as usize || change.swap_slot == Some(slot as usize)
            }) {
                let change = self.wall_change.take().unwrap();
                if change.started.elapsed() >= Duration::from_millis(400) {
                    if let Some(other) = change.swap_slot {
                        self.wall.swap(change.slot, other);
                    } else {
                        self.wall[change.slot] = change.next;
                    }
                }
                self.wall_next = Instant::now() + Duration::from_secs_f32(self.settings.seconds());
            }
            self.wall_slot = slot as usize;
            self.selected = self.wall[self.wall_slot];
            self.request_around();
        }
    }

    fn update_wall(&mut self, now: Instant) {
        if self.wall.len() < 2 || self.settings.effect == 4 {
            return;
        }
        if let Some(change) = &self.wall_change {
            if now.duration_since(change.started) >= Duration::from_millis(800) {
                if let Some(other) = change.swap_slot {
                    self.wall.swap(change.slot, other);
                } else {
                    self.wall[change.slot] = change.next;
                }
                self.selected = self.wall[self.wall_slot];
                self.wall_change = None;
                self.wall_next = now + Duration::from_secs_f32(self.settings.seconds());
                self.request_around();
            }
            return;
        }
        if let Some((slot, next, requested)) = self.wall_pending {
            if self.covers[next].is_some()
                || self.tx_req.is_none()
                || now.duration_since(requested) >= Duration::from_secs(5)
            {
                self.wall_change = Some(WallChange {
                    slot,
                    next,
                    started: now,
                    effect: if self.settings.effect == 3 {
                        self.random(3) as u8
                    } else {
                        self.settings.effect
                    },
                    swap_slot: None,
                });
                self.wall_pending = None;
            }
            return;
        }
        if now < self.wall_next {
            return;
        }
        let slot = self.random(self.wall.len());
        if self.albums.len() <= self.wall.len() {
            if self.wall.len() < 2 {
                self.wall_next = now + Duration::from_secs_f32(self.settings.seconds());
                return;
            }
            let mut other = self.random(self.wall.len() - 1);
            if other >= slot {
                other += 1;
            }
            self.wall_change = Some(WallChange {
                slot,
                next: self.wall[other],
                started: now,
                effect: if self.settings.effect == 3 {
                    self.random(3) as u8
                } else {
                    self.settings.effect
                },
                swap_slot: Some(other),
            });
            return;
        }
        let mut next = self.random(self.albums.len());
        while self.wall.contains(&next) {
            next = (next + 1) % self.albums.len();
        }
        self.wall_pending = Some((slot, next, now));
        self.request_around();
        if let Some(tx) = &self.tx_req {
            if self.covers[next].is_none() && self.requested.insert(next) {
                let _ = tx.send(next);
            }
        }
    }

    fn scroll_by(&mut self, dir: i8, weight: f32) {
        let now = Instant::now();
        if dir != self.scroll_dir {
            self.scroll_accum = 0.0;
            self.scroll_dir = dir;
        }
        self.scroll_accum += dir as f32 * weight;
        self.scroll_last_event = now;
        let threshold = self.settings.scroll_events();
        let cooldown = Duration::from_millis(self.settings.scroll_cooldown_ms());
        if self.scroll_accum.abs() >= threshold
            && now.duration_since(self.scroll_last_move) >= cooldown
        {
            if dir < 0 {
                self.selected = self.selected.saturating_sub(1);
            } else {
                self.selected = (self.selected + 1).min(self.albums.len().saturating_sub(1));
            }
            self.scroll_accum -= dir as f32 * threshold;
            self.scroll_last_move = now;
            self.request_around();
        }
    }

    fn keep_cover(&self, i: usize) -> bool {
        (i as isize - self.selected as isize).abs() <= 48
            || (self.mode == ViewMode::Grid
                && (self.wall.contains(&i)
                    || self
                        .wall_pending
                        .as_ref()
                        .is_some_and(|&(_, next, _)| next == i)
                    || self
                        .wall_change
                        .as_ref()
                        .is_some_and(|change| change.next == i)))
    }

    fn request_around(&mut self) {
        // keep memory bounded while browsing a large library
        for i in 0..self.covers.len() {
            if self.covers[i].is_some() && !self.keep_cover(i) {
                self.covers[i] = None;
                self.pngs[i] = None;
                self.sprites[i] = None;
                let flat_id = 2000 + i as u32;
                if self.uploaded.remove(&flat_id) {
                    self.pending_deletions.push((i, flat_id));
                }
                for bin in 0..sprite::BINS {
                    let id = 6000 + i as u32 * sprite::BINS as u32 + bin as u32;
                    if self.uploaded.remove(&id) {
                        self.pending_deletions.push((i, id));
                    }
                }
                self.requested.remove(&i);
                self.sprites_requested.retain(|&(idx, _)| idx != i);
            }
        }
        let Some(tx) = &self.tx_req else {
            self.request_flow_sprites();
            return;
        };
        let n = self.albums.len();
        if n == 0 {
            return;
        }
        let mut wanted = HashSet::new();
        if self.mode == ViewMode::Grid {
            wanted.extend(self.wall.iter().copied());
            if let Some((_, next, _)) = self.wall_pending {
                wanted.insert(next);
            }
            if let Some(change) = &self.wall_change {
                wanted.insert(change.next);
            }
            wanted.insert(self.selected);
        } else {
            let lo = self.selected.saturating_sub(8);
            let hi = (self.selected + 9).min(n);
            wanted.extend(lo..hi);
        }
        *self.wanted.lock().unwrap() = wanted;
        if self.mode == ViewMode::Grid {
            for &i in &self.wall {
                if self.covers[i].is_none() && self.requested.insert(i) {
                    let _ = tx.send(i);
                }
            }
        }
        let radius = if self.mode == ViewMode::Grid { 0 } else { 8 };
        // Visible covers first, then progressively farther prefetches.
        for distance in 0..=radius {
            for i in [
                self.selected.checked_add(distance),
                self.selected.checked_sub(distance),
            ] {
                if let Some(i) = i.filter(|&i| i < n) {
                    if self.covers[i].is_none() && self.requested.insert(i) {
                        let _ = tx.send(i);
                    }
                }
            }
        }
        self.request_flow_sprites();
    }

    fn request_flow_sprites(&mut self) {
        if !self.kitty || self.mode != ViewMode::Flow {
            self.sprite_wanted.lock().unwrap().clear();
            return;
        }
        let Some(tx) = &self.tx_sprites else { return };
        let mut visible = Vec::new();
        for distance in 0..=8 {
            for i in [
                self.selected.checked_add(distance),
                self.selected.checked_sub(distance),
            ] {
                if let Some(i) = i.filter(|&i| i < self.covers.len()) {
                    if (i as f32 - self.pos).abs() <= 7.0 && !visible.contains(&i) {
                        visible.push(i);
                    }
                }
            }
        }
        // First make every visible cover available at its current angle.
        let mut jobs: Vec<(usize, usize)> = visible
            .iter()
            .map(|&i| (i, sprite_bin(i as f32 - self.pos)))
            .collect();
        // Then prewarm the selected cover and its neighbours, interleaving
        // nearby angles so the next interaction does not wait for the worker.
        let neighbours: Vec<usize> = visible
            .iter()
            .copied()
            .filter(|&i| i.abs_diff(self.selected) <= 1)
            .collect();
        for offset in 1..sprite::BINS {
            for &i in &neighbours {
                let base = sprite_bin(i as f32 - self.pos);
                if let Some(bin) = base.checked_sub(offset) {
                    jobs.push((i, bin));
                }
                if base + offset < sprite::BINS {
                    jobs.push((i, base + offset));
                }
            }
        }
        *self.sprite_wanted.lock().unwrap() = jobs.iter().copied().collect();
        for (i, bin) in jobs {
            let ready = self.sprites[i]
                .as_ref()
                .and_then(|bins| bins.get(bin))
                .is_some_and(|png| !png.is_empty());
            let cover = self.covers[i].as_ref().or_else(|| {
                if self.tx_req.is_none() {
                    self.fallback.get(i)
                } else {
                    None
                }
            });
            if !ready {
                if let Some(cover) = cover {
                    if self.sprites_requested.insert((i, bin)) {
                        let _ = tx.send((i, bin, Arc::clone(&cover.px), cover.w as u32));
                    }
                }
            }
        }
    }

    fn poll(&mut self) -> bool {
        let Some(rx) = self.rx.take() else {
            return false;
        };
        let mut changed = false;
        let mut retry_sprite = false;
        loop {
            match rx.try_recv() {
                Ok(Msg::Albums(albums)) => {
                    changed = true;
                    self.fallback = (0..albums.len().min(100)).map(fallback_cover).collect();
                    self.covers = (0..albums.len()).map(|_| None).collect();
                    self.pngs = (0..albums.len()).map(|_| None).collect();
                    self.sprites = (0..albums.len()).map(|_| None).collect();
                    self.albums = albums;
                    self.status = format!("{} albums   ← → browse", self.albums.len());
                    let start = std::env::var("TERA_START")
                        .ok()
                        .and_then(|s| s.parse().ok());
                    self.restore_selection(start);
                    self.request_around();
                }
                Ok(Msg::Art {
                    idx,
                    px,
                    png,
                    sprites,
                }) => {
                    changed = true;
                    if idx < self.covers.len() && self.keep_cover(idx) {
                        self.covers[idx] = Some(Cover {
                            px: Arc::new(px),
                            w: ART_SIZE as usize,
                            h: ART_SIZE as usize,
                        });
                        self.pngs[idx] = png;
                        self.sprites[idx] = sprites;
                        self.pending_upload.push(idx);
                        self.request_flow_sprites();
                    } else {
                        self.requested.remove(&idx);
                    }
                }
                Ok(Msg::Sprite { idx, bin, png }) => {
                    changed = true;
                    if idx < self.covers.len()
                        && (self.covers[idx].is_some() || self.tx_req.is_none())
                    {
                        let bins =
                            self.sprites[idx].get_or_insert_with(|| vec![Vec::new(); sprite::BINS]);
                        bins[bin] = png;
                    } else {
                        self.sprites_requested.remove(&(idx, bin));
                    }
                }
                Ok(Msg::SpriteSkipped(idx, bin)) => {
                    self.sprites_requested.remove(&(idx, bin));
                    retry_sprite |= self.sprite_wanted.lock().unwrap().contains(&(idx, bin));
                }
                Ok(Msg::Skipped(idx)) => {
                    self.requested.remove(&idx);
                    let needed = self.wanted.lock().unwrap().contains(&idx);
                    if needed {
                        self.request_around();
                    }
                }
                Ok(Msg::Error(e)) => {
                    changed = true;
                    self.status = format!("music error: {e}");
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
        self.rx = Some(rx);
        if retry_sprite {
            self.request_flow_sprites();
        }
        changed
    }
}

fn loader(tx: Sender<Msg>, rx_req: Receiver<usize>, wanted: Arc<Mutex<HashSet<usize>>>) {
    let albums = match music::load_albums() {
        Ok(a) => a,
        Err(e) => {
            let _ = tx.send(Msg::Error(e.to_string()));
            return;
        }
    };
    let _ = tx.send(Msg::Albums(albums.clone()));
    let fetch = |i: usize| {
        if i >= albums.len() {
            return;
        }
        let pid = &albums[i].pid;
        if pid.is_empty() {
            return;
        }
        if let Some(px) = music::load_cover(pid, ART_SIZE) {
            let png = kitty::encode_cover_png(&px, ART_SIZE, ART_SIZE);
            let sprites = None;
            let _ = tx.send(Msg::Art {
                idx: i,
                px,
                png,
                sprites,
            });
        }
    };
    while let Ok(i) = rx_req.recv() {
        let needed = wanted.lock().unwrap().contains(&i);
        if needed {
            fetch(i);
        } else if tx.send(Msg::Skipped(i)).is_err() {
            break;
        }
    }
}

fn spawn_sprite_worker(
    tx: Sender<Msg>,
    wanted: Arc<Mutex<HashSet<(usize, usize)>>>,
) -> Sender<(usize, usize, Arc<Vec<[u8; 3]>>, u32)> {
    let (requests, rx) = mpsc::channel::<(usize, usize, Arc<Vec<[u8; 3]>>, u32)>();
    let rx = Arc::new(Mutex::new(rx));
    for _ in 0..2 {
        let tx = tx.clone();
        let rx = Arc::clone(&rx);
        let wanted = Arc::clone(&wanted);
        thread::spawn(move || {
            loop {
                let job = rx.lock().unwrap().recv();
                let Ok((idx, bin, px, size)) = job else { break };
                let needed = wanted.lock().unwrap().contains(&(idx, bin));
                if !needed {
                    if tx.send(Msg::SpriteSkipped(idx, bin)).is_err() {
                        break;
                    }
                    continue;
                }
                let card =
                    sprite::warp_rgba(&px, size, sprite::yaw_for_bin(bin), sprite::SPRITE_SIZE);
                let (raw, height) = sprite::with_reflection(
                    &card,
                    sprite::SPRITE_SIZE,
                    (sprite::SPRITE_SIZE as f32 * 0.42) as u32,
                );
                let padding = kitty::cover_padding(sprite::SPRITE_SIZE);
                let padded = sprite::pad_right(&raw, sprite::SPRITE_SIZE, height, padding);
                let png = sprite::encode_rgba_wh(&padded, sprite::SPRITE_SIZE + padding, height)
                    .unwrap_or_default();
                if tx.send(Msg::Sprite { idx, bin, png }).is_err() {
                    break;
                }
            }
        });
    }
    requests
}

fn flow_focus(distance: f32) -> f32 {
    let t = distance.abs().min(1.0);
    t * t * (3.0 - 2.0 * t)
}

fn flow_yaw(distance: f32) -> f32 {
    -distance.signum() * 1.05 * flow_focus(distance)
}

fn sprite_bin(distance: f32) -> usize {
    let yaw = flow_yaw(distance);
    (((yaw + 1.05) / (2.1 / (sprite::BINS - 1) as f32)).round() as usize).min(sprite::BINS - 1)
}

fn flow_crop_width(
    visible_source_w: u32,
    source_w: u32,
    crop_x: u32,
    desired_pixels: f32,
    available_pixels: f32,
) -> u32 {
    ((visible_source_w as f32 * available_pixels / desired_pixels).round() as u32)
        .max(visible_source_w)
        .min(source_w + kitty::cover_padding(source_w) - crop_x)
}

fn closest_ready_sprite(bins: &[Vec<u8>], requested: usize) -> Option<usize> {
    bins.iter()
        .enumerate()
        .filter(|(_, png)| !png.is_empty())
        .min_by_key(|(bin, _)| bin.abs_diff(requested))
        .map(|(bin, _)| bin)
}

fn update_placement<W: Write>(
    out: &mut W,
    old: Option<&ActivePlacement>,
    new: &ActivePlacement,
    pid: u32,
) -> io::Result<()> {
    if old == Some(new) {
        return Ok(());
    }
    if let Some(old) = old {
        if old.image_id != new.image_id {
            // Both images can temporarily coexist: show the new one before
            // removing the old, so the cover is never absent between frames.
            out.write_all(new.command.as_bytes())?;
            out.write_all(kitty::delete_placement(old.image_id, pid).as_bytes())?;
            return Ok(());
        }
        if old.z != new.z {
            out.write_all(kitty::delete_placement(old.image_id, pid).as_bytes())?;
        }
    }
    out.write_all(new.command.as_bytes())
}

fn fallback_cover(idx: usize) -> Cover {
    let w = 64usize;
    let h = 64usize;
    let mut px = vec![[0u8; 3]; w * h];
    let hue = (idx as f32 * 0.13) % 1.0;
    let (r0, g0, b0) = hsv(hue, 0.55, 0.95);
    let (r1, g1, b1) = hsv((hue + 0.12) % 1.0, 0.85, 0.35);
    for y in 0..h {
        for x in 0..w {
            let u = x as f32 / (w - 1) as f32;
            let v = y as f32 / (h - 1) as f32;
            let t = (u * 0.65 + v * 0.35).clamp(0.0, 1.0);
            let mut r = lerp(r0, r1, t);
            let mut g = lerp(g0, g1, t);
            let mut b = lerp(b0, b1, t);
            let d = (u - v + 1.0) * 0.5;
            if (d - 0.5).abs() < 0.035 {
                r = (r as f32 * 0.4 + 255.0 * 0.6) as u8;
                g = (g as f32 * 0.4 + 255.0 * 0.6) as u8;
                b = (b as f32 * 0.4 + 255.0 * 0.6) as u8;
            }
            let dx = u - 0.5;
            let dy = v - 0.5;
            let vg = 1.0 - (dx * dx + dy * dy) * 0.85;
            r = (r as f32 * vg).clamp(0.0, 255.0) as u8;
            g = (g as f32 * vg).clamp(0.0, 255.0) as u8;
            b = (b as f32 * vg).clamp(0.0, 255.0) as u8;
            px[y * w + x] = [r, g, b];
        }
    }
    Cover {
        px: Arc::new(px),
        w,
        h,
    }
}

fn lerp(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t).clamp(0.0, 255.0) as u8
}

fn hsv(h: f32, s: f32, v: f32) -> (u8, u8, u8) {
    let i = (h * 6.0).floor() as i32;
    let f = h * 6.0 - i as f32;
    let p = v * (1.0 - s);
    let q = v * (1.0 - f * s);
    let t = v * (1.0 - (1.0 - f) * s);
    let (r, g, b) = match i.rem_euclid(6) {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    ((r * 255.0) as u8, (g * 255.0) as u8, (b * 255.0) as u8)
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--dump") {
        return dump_scene();
    }
    if args.iter().any(|a| a == "--haptic-test") {
        let mut h = haptic::Haptics::new(true);
        for _ in 0..10 {
            h.tick();
            std::thread::sleep(Duration::from_millis(200));
        }
        return Ok(());
    }
    if args.iter().any(|a| a == "--bench") {
        let frames: usize = args
            .iter()
            .position(|a| a == "--bench")
            .and_then(|i| args.get(i + 1))
            .and_then(|s| s.parse().ok())
            .unwrap_or(600);
        return bench(frames);
    }
    run_tui(
        args.iter().any(|a| a == "--synthetic"),
        args.iter().any(|a| a == "--no-kitty"),
        args.iter().any(|a| a == "--mute"),
        args.iter().any(|a| a == "--no-haptic"),
    )
}

fn cell_pixels() -> (u16, u16) {
    if let Ok(ws) = terminal::window_size() {
        if ws.columns > 0 && ws.rows > 0 && ws.width > 0 && ws.height > 0 {
            return ((ws.width / ws.columns).max(1), (ws.height / ws.rows).max(1));
        }
    }
    (8, 16)
}

fn dump_scene() -> Result<()> {
    let mut app = App::synthetic();
    app.kitty = false;
    if let Ok(albums) = music::load_albums() {
        app.fallback = (0..albums.len().min(100)).map(fallback_cover).collect();
        app.covers = (0..albums.len()).map(|_| None).collect();
        for i in 0..albums.len().min(8) {
            if let Some(px) = music::load_cover(&albums[i].pid, ART_SIZE) {
                app.covers[i] = Some(Cover {
                    px: Arc::new(px),
                    w: ART_SIZE as usize,
                    h: ART_SIZE as usize,
                });
            }
        }
        app.albums = albums;
    }
    let area = Rect::new(0, 0, 120, 40);
    let mut buf = Buffer::empty(area);
    draw_scene(&mut buf, &app, area);
    let mut out = String::new();
    for y in 0..area.height {
        for x in 0..area.width {
            let c = &buf[(x, y)];
            let (fr, fg, fb) = rgb(c.fg);
            let (br, bg, bb) = rgb(c.bg);
            out.push_str(&format!(
                "{},{},{},{},{},{},{},{},{}\n",
                x,
                y,
                c.symbol(),
                fr,
                fg,
                fb,
                br,
                bg,
                bb
            ));
        }
    }
    std::fs::write("/tmp/tera_dump.csv", out)?;
    println!("dumped {} albums", app.albums.len());
    Ok(())
}

fn run_tui(synthetic: bool, no_kitty: bool, mute: bool, no_haptic: bool) -> Result<()> {
    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        terminal::EnterAlternateScreen,
        EnableMouseCapture,
        cursor::Hide,
        crossterm::style::ResetColor,
        Clear(ClearType::All)
    )?;
    let mut app = if synthetic {
        App::synthetic()
    } else {
        App::with_music()
    };
    app.settings = settings::Settings::load();
    app.session = session::Session::load(synthetic);
    app.mode = if app.session.grid {
        ViewMode::Grid
    } else {
        ViewMode::Flow
    };
    app.forced_blocks = no_kitty;
    app.kitty = kitty::supported() && app.settings.hd && !app.forced_blocks;
    if synthetic && kitty::supported() {
        let (tx, rx) = mpsc::channel();
        app.tx_sprites = Some(spawn_sprite_worker(tx, Arc::clone(&app.sprite_wanted)));
        app.rx = Some(rx);
        for i in 0..app.albums.len() {
            let cover = &app.fallback[i % app.fallback.len()];
            app.pngs[i] = kitty::encode_cover_png(&cover.px, cover.w as u32, cover.h as u32);
            app.pending_upload.push(i);
        }
    }
    match std::env::var("TERA_VIEW").as_deref() {
        Ok("grid") => app.mode = ViewMode::Grid,
        Ok("flow") => app.mode = ViewMode::Flow,
        _ => {}
    }
    if synthetic {
        let start = std::env::var("TERA_START")
            .ok()
            .and_then(|s| s.parse().ok());
        app.restore_selection(start);
        app.request_flow_sprites();
    }
    let mut saved_session = app.session.clone();
    let mut session_dirty_since: Option<Instant> = None;
    let size = terminal::size()?;
    let mut area = Rect::new(0, 0, size.0, size.1);
    let mut buf = Buffer::empty(area);
    let mut prev = Buffer::empty(area);
    let mut last = Instant::now();
    let mut acc = 0.0f64;
    let mut frames = 0u32;
    let demo = std::env::var("TERA_DEMO").is_ok();
    let mut demo_next = Instant::now() + Duration::from_millis(1200);
    let mut out = BufWriter::with_capacity(1 << 20, io::stdout());
    let mut visible_prev: HashSet<usize> = HashSet::new();
    let mut overlay_image: Option<(u32, u32)> = None;
    let click_on = !mute
        && std::env::var("TERA_CLICK")
            .map(|v| v != "0" && v != "off")
            .unwrap_or(true);
    let volume = std::env::var("TERA_CLICK_VOLUME")
        .ok()
        .and_then(|v| v.parse::<f32>().ok())
        .unwrap_or(0.22);
    let clicker = if click_on {
        click::Clicker::new(volume)
    } else {
        None
    };
    let haptic_on = !no_haptic
        && std::env::var("TERA_HAPTIC")
            .map(|v| v != "0" && v != "off")
            .unwrap_or(true);
    let mut haptics = haptic::Haptics::new(haptic_on);
    let mut last_tick = app.pos.round() as i32;
    let mut last_click = Instant::now();
    let mut frame_dirty = true;
    let (playback_tx, playback_commands) = mpsc::channel::<PlaybackCommand>();
    let (playback_results, playback_rx) = mpsc::channel::<bool>();
    thread::spawn(move || {
        while let Ok(command) = playback_commands.recv() {
            match command {
                PlaybackCommand::Toggle => music::play_pause(),
                PlaybackCommand::Play(pid) => music::play_track(&pid),
            }
            if playback_results.send(music::is_playing()).is_err() {
                break;
            }
        }
    });

    'outer: loop {
        while event::poll(Duration::from_secs(0))? {
            frame_dirty = true;
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press && app.settings_open => {
                    match k.code {
                        KeyCode::Esc | KeyCode::Char('s') | KeyCode::Char(',') => {
                            app.settings_open = false
                        }
                        KeyCode::Up | KeyCode::Char('k') => {
                            app.settings_row = app.settings_row.saturating_sub(1)
                        }
                        KeyCode::Down | KeyCode::Char('j') => {
                            app.settings_row = (app.settings_row + 1).min(7)
                        }
                        KeyCode::Left | KeyCode::Char('h') => change_setting(&mut app, -1),
                        KeyCode::Right
                        | KeyCode::Char('l')
                        | KeyCode::Enter
                        | KeyCode::Char(' ') => change_setting(&mut app, 1),
                        _ => {}
                    }
                }
                Event::Key(k)
                    if k.kind == KeyEventKind::Press
                        && matches!(k.code, KeyCode::Char('s') | KeyCode::Char(',')) =>
                {
                    app.settings_open = true
                }
                Event::Key(k) if k.kind == KeyEventKind::Press && app.detail_open => match k.code {
                    KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('i') => {
                        app.detail_open = false;
                        if app.mode == ViewMode::Grid || !app.settings.album_flip {
                            app.flip = 0.0;
                        }
                    }
                    KeyCode::Char('q') => break 'outer,
                    KeyCode::Up | KeyCode::Char('k') => {
                        app.track_selected = app.track_selected.saturating_sub(1)
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        app.track_selected =
                            (app.track_selected + 1).min(app.tracks.len().saturating_sub(1))
                    }
                    KeyCode::Enter => {
                        if let Some(track) = app.tracks.get(app.track_selected) {
                            let _ = playback_tx.send(PlaybackCommand::Play(track.pid.clone()));
                            app.playing = true;
                        }
                    }
                    _ => {}
                },
                Event::Key(k) if k.kind == KeyEventKind::Press => match k.code {
                    KeyCode::Char('q') | KeyCode::Esc => break 'outer,
                    KeyCode::Left | KeyCode::Char('h')
                        if app.mode == ViewMode::Flow && app.selected > 0 =>
                    {
                        app.move_flow(-1);
                    }
                    KeyCode::Right | KeyCode::Char('l')
                        if app.mode == ViewMode::Flow && app.selected + 1 < app.albums.len() =>
                    {
                        app.move_flow(1);
                    }
                    KeyCode::Tab => {
                        app.record_session();
                        app.mode = if app.mode == ViewMode::Flow {
                            ViewMode::Grid
                        } else {
                            ViewMode::Flow
                        };
                        if app.mode == ViewMode::Grid {
                            app.sync_wall(area);
                        }
                        app.request_around();
                    }
                    KeyCode::Char(' ') => {
                        let _ = playback_tx.send(PlaybackCommand::Toggle);
                        app.playing = !app.playing;
                    }
                    KeyCode::Enter | KeyCode::Char('i') if app.mode == ViewMode::Flow => {
                        app.open_detail()
                    }
                    KeyCode::Home if app.mode == ViewMode::Flow => {
                        app.selected = 0;
                        app.request_around();
                    }
                    KeyCode::End if app.mode == ViewMode::Flow => {
                        app.selected = app.albums.len().saturating_sub(1);
                        app.request_around();
                    }
                    _ => {}
                },
                Event::Mouse(m)
                    if app.settings_open
                        && matches!(m.kind, MouseEventKind::Down(event::MouseButton::Left)) =>
                {
                    let rect = settings_rect(area);
                    let first = rect.y.saturating_add(3);
                    if m.column > rect.x
                        && m.column < rect.right().saturating_sub(1)
                        && m.row >= first
                        && m.row < first.saturating_add(8)
                    {
                        app.settings_row = (m.row - first) as usize;
                        change_setting(&mut app, 1);
                    }
                }
                Event::Mouse(_) if app.settings_open => {}
                Event::Mouse(m)
                    if matches!(m.kind, MouseEventKind::Down(event::MouseButton::Left)) =>
                {
                    if app.detail_open && app.flip > 0.95 {
                        let panel = detail_rect(area);
                        let start = detail_list_top(panel);
                        let visible = detail_list_capacity(panel);
                        let first = app.track_selected.saturating_sub(visible.saturating_sub(1));
                        if m.column > panel.x
                            && m.column < panel.right().saturating_sub(1)
                            && m.row >= start
                            && m.row < start.saturating_add(visible as u16)
                        {
                            let index = first + (m.row - start) as usize;
                            if index < app.tracks.len() {
                                if index == app.track_selected {
                                    let _ = playback_tx
                                        .send(PlaybackCommand::Play(app.tracks[index].pid.clone()));
                                    app.playing = true;
                                } else {
                                    app.track_selected = index;
                                }
                            }
                        }
                    } else if !app.detail_open && app.mode == ViewMode::Grid {
                        let (cols, rows, cell_w, cell_h, _) = grid_metrics(area);
                        let mouse_x = m.column as f32 - grid_left(area, cols, cell_w);
                        let column = (mouse_x / cell_w) as usize;
                        let row = (m.row as f32 / cell_h) as usize;
                        let slot = column * rows + row;
                        if mouse_x >= 0.0
                            && column < cols
                            && row < rows
                            && app.wall.get(slot).is_some()
                        {
                            app.move_wall_selection(slot as isize - app.wall_slot as isize);
                            app.open_detail();
                        }
                    } else if !app.detail_open && app.mode == ViewMode::Flow {
                        let placements = draw_scene(&mut buf, &app, area);
                        if placements.iter().any(|&(i, x, y, w, h, _)| {
                            i == app.selected
                                && (m.column as f32) >= x
                                && (m.column as f32) < x + w
                                && (m.row as f32) >= y
                                && (m.row as f32) < y + h
                        }) {
                            app.open_detail();
                        }
                    }
                }
                Event::Mouse(m)
                    if app.detail_open
                        && matches!(
                            m.kind,
                            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown
                        ) =>
                {
                    if m.kind == MouseEventKind::ScrollUp {
                        app.track_selected = app.track_selected.saturating_sub(1);
                    } else {
                        app.track_selected =
                            (app.track_selected + 1).min(app.tracks.len().saturating_sub(1));
                    }
                }
                Event::Mouse(m)
                    if matches!(
                        m.kind,
                        MouseEventKind::ScrollUp
                            | MouseEventKind::ScrollDown
                            | MouseEventKind::ScrollLeft
                            | MouseEventKind::ScrollRight
                    ) =>
                {
                    if app.mode == ViewMode::Grid {
                        continue;
                    }
                    let dir: i8 = if matches!(
                        m.kind,
                        MouseEventKind::ScrollUp | MouseEventKind::ScrollLeft
                    ) {
                        -1
                    } else {
                        1
                    };
                    let weight = if matches!(
                        m.kind,
                        MouseEventKind::ScrollLeft | MouseEventKind::ScrollRight
                    ) {
                        0.5
                    } else {
                        1.0
                    };
                    app.scroll_by(dir, weight);
                }
                Event::Resize(w, h) => {
                    if app.kitty {
                        out.write_all(kitty::clear_placements().as_bytes())?;
                        app.active_image.clear();
                        visible_prev.clear();
                        if overlay_image.take().is_some() {
                            out.write_all(kitty::delete_image(3).as_bytes())?;
                        }
                    }
                    area = Rect::new(0, 0, w, h);
                    if app.mode == ViewMode::Grid {
                        app.sync_wall(area);
                        app.request_around();
                    }
                    buf = Buffer::empty(area);
                    prev = Buffer::empty(area);
                    execute!(out, crossterm::style::ResetColor, Clear(ClearType::All))?;
                    out.flush()?;
                }
                _ => {}
            }
        }

        let want_kitty = app.settings.hd && !app.forced_blocks && kitty::supported();
        if want_kitty != app.kitty {
            frame_dirty = true;
            out.write_all(kitty::clear_placements().as_bytes())?;
            for id in app.uploaded.drain() {
                out.write_all(kitty::delete_image(id).as_bytes())?;
            }
            if overlay_image.is_some() {
                out.write_all(kitty::delete_image(3).as_bytes())?;
            }
            for (_, id) in app.pending_deletions.drain(..) {
                out.write_all(kitty::delete_image(id).as_bytes())?;
            }
            app.active_image.clear();
            overlay_image = None;
            visible_prev.clear();
            app.pending_upload = app
                .pngs
                .iter()
                .enumerate()
                .filter_map(|(i, p)| p.as_ref().map(|_| i))
                .collect();
            app.kitty = want_kitty;
            if app.kitty {
                app.request_flow_sprites();
            }
        }
        frame_dirty |= app.poll();
        frame_dirty |= app.poll_tracks();
        while let Ok(playing) = playback_rx.try_recv() {
            app.playing = playing;
            frame_dirty = true;
        }
        let now = Instant::now();
        if demo
            && app.mode == ViewMode::Flow
            && !app.detail_open
            && !app.settings_open
            && app.flip == 0.0
            && now >= demo_next
            && !app.albums.is_empty()
        {
            app.selected = (app.selected + 1) % app.albums.len();
            frame_dirty = true;
            app.request_around();
            demo_next = now + Duration::from_millis(1200);
        }
        let dt = now.duration_since(last).as_secs_f32();
        let old_flip = app.flip;
        let old_pos = app.pos;
        let wall_was_animating = app.wall_change.is_some();
        app.flip = if app.mode == ViewMode::Grid || !app.settings.album_flip {
            if app.detail_open { 1.0 } else { 0.0 }
        } else if app.detail_open {
            (app.flip + dt / 0.48).min(1.0)
        } else {
            (app.flip - dt / 0.48).max(0.0)
        };
        if app.mode == ViewMode::Grid
            && !app.detail_open
            && !app.settings_open
            && app.flip == 0.0
            && !app.albums.is_empty()
        {
            let old_len = app.wall.len();
            app.sync_wall(area);
            if app.wall.len() != old_len {
                app.request_around();
            }
            app.update_wall(now);
        }
        last = now;
        let target = app.selected as f32;
        if (app.pos - target).abs() > 0.0005 {
            app.pos += (target - app.pos) * (1.0 - (-dt / 0.10).exp());
        } else {
            app.pos = target;
        }
        let wall_visible = app.mode == ViewMode::Grid && !app.detail_open && !app.settings_open;
        if app.pos != old_pos {
            app.request_flow_sprites();
        }
        frame_dirty |= app.flip != old_flip
            || app.pos != old_pos
            || (wall_visible && (wall_was_animating || app.wall_change.is_some()));
        frame_dirty |=
            app.kitty && (!app.pending_upload.is_empty() || !app.pending_deletions.is_empty());
        if now.duration_since(app.scroll_last_event) > Duration::from_millis(320) {
            app.scroll_accum = 0.0;
        }
        let tick = if app.mode == ViewMode::Grid {
            app.selected as i32
        } else {
            app.pos.round() as i32
        };
        if tick != last_tick {
            if now.duration_since(last_click) >= Duration::from_millis(35) {
                if let Some(c) = &clicker {
                    c.play();
                }
                last_click = now;
            }
            haptics.tick();
            last_tick = tick;
        }

        if frame_dirty {
            let placements = draw_scene(&mut buf, &app, area);
            if app.settings_open {
                draw_settings(&mut buf, &app, area);
            }
            if std::env::var("TERA_NOSYNC").is_err() {
                queue!(out, BeginSynchronizedUpdate)?;
            }
            encode_diff(&mut out, &prev, &buf);
            if app.kitty {
                use std::io::Write as _;
                for (idx, id) in app.pending_deletions.drain(..) {
                    out.write_all(kitty::delete_image(id).as_bytes())?;
                    if app
                        .active_image
                        .get(&idx)
                        .is_some_and(|placement| placement.image_id == id)
                    {
                        app.active_image.remove(&idx);
                    }
                }
                let uploads = app.pending_upload.len().min(2);
                for idx in app.pending_upload.drain(..uploads) {
                    if let Some(Some(png)) = app.pngs.get(idx) {
                        let id = 2000 + idx as u32;
                        if app.active_image.get(&idx).is_some_and(|p| p.image_id == id) {
                            out.write_all(
                                kitty::delete_placement(id, 3000 + idx as u32).as_bytes(),
                            )?;
                            app.active_image.remove(&idx);
                        }
                        out.write_all(kitty::upload(id, png).as_bytes())?;
                        app.uploaded.insert(2000 + idx as u32);
                    }
                }
                let (cell_w, cell_h) = cell_pixels();
                let mut visible_now: HashSet<usize> = HashSet::new();
                for (idx, x0, y0, cw, ch, bin) in placements {
                    let flat = app.pngs.get(idx).and_then(|p| p.as_ref());
                    let sprite = if app.mode == ViewMode::Flow && app.flip == 0.0 {
                        app.sprites
                            .get(idx)
                            .and_then(|v| v.as_ref())
                            .and_then(|bins| {
                                closest_ready_sprite(bins, bin).map(|ready| (ready, &bins[ready]))
                            })
                    } else {
                        None
                    };
                    let (image_id, bytes) = if let Some((ready, spr)) = sprite {
                        (
                            6000 + idx as u32 * sprite::BINS as u32 + ready as u32,
                            Some(spr),
                        )
                    } else {
                        let id = 2000 + idx as u32;
                        if flat.is_none() && !app.uploaded.contains(&id) {
                            let cover = &app.fallback[idx % app.fallback.len()];
                            if let Some(png) =
                                kitty::encode_cover_png(&cover.px, cover.w as u32, cover.h as u32)
                            {
                                out.write_all(kitty::upload(id, &png).as_bytes())?;
                                app.uploaded.insert(id);
                            }
                        }
                        (id, None)
                    };
                    if image_id >= 6000 && !app.uploaded.contains(&image_id) {
                        if let Some(bytes) = bytes {
                            out.write_all(kitty::upload(image_id, bytes).as_bytes())?;
                            app.uploaded.insert(image_id);
                        }
                    }
                    let z = placement_z(idx, app.pos, app.mode, app.flip, app.settings_open);
                    let place_ch = if image_id >= 6000 && app.settings.reflection {
                        ch * 1.42
                    } else {
                        ch
                    };
                    let left = x0.max(0.0);
                    let top = y0.max(0.0);
                    let right = (x0 + cw).min(area.width as f32);
                    let bottom = (y0 + place_ch).min(area.height as f32);
                    if right <= left || bottom <= top {
                        continue;
                    }
                    let source_w = if image_id >= 6000 {
                        sprite::SPRITE_SIZE
                    } else if synthetic || flat.is_none() {
                        64
                    } else {
                        ART_SIZE
                    };
                    let source_h = if image_id >= 6000 && app.settings.reflection {
                        source_w + (source_w as f32 * 0.42) as u32
                    } else {
                        source_w
                    };
                    let crop_x = ((left - x0) / cw * source_w as f32) as u32;
                    let crop_y = ((top - y0) / place_ch * source_h as f32) as u32;
                    let visible_source_w = (((right - left) / cw * source_w as f32).round() as u32)
                        .max(1)
                        .min(source_w - crop_x);
                    let crop_h = (((bottom - top) / place_ch * source_h as f32) as u32)
                        .max(1)
                        .min(source_h - crop_y);
                    let col = left as u16;
                    let row = top as u16;
                    let xoff = ((left - col as f32) * cell_w as f32) as u16;
                    let yoff = ((top - row as f32) * cell_h as f32) as u16;
                    let flow_placement = app.mode == ViewMode::Flow && app.flip == 0.0;
                    let cols = if app.mode == ViewMode::Grid || flow_placement {
                        (right - left + xoff as f32 / cell_w as f32).ceil()
                    } else {
                        (right - left).round()
                    }
                    .max(1.0) as u16;
                    let crop_w = if flow_placement {
                        let desired_pixels = (right - left) * cell_w as f32;
                        let available_pixels = cols as f32 * cell_w as f32 - xoff as f32;
                        flow_crop_width(
                            visible_source_w,
                            source_w,
                            crop_x,
                            desired_pixels,
                            available_pixels,
                        )
                    } else {
                        visible_source_w
                    };
                    let command = kitty::place_crop(
                        image_id,
                        3000 + idx as u32,
                        col,
                        row,
                        cols,
                        if flow_placement {
                            // Let Kitty derive the height from the image aspect ratio.
                            // The square cover stays the same size with or without its reflection.
                            None
                        } else {
                            Some(
                                if app.mode == ViewMode::Grid {
                                    (bottom - top + yoff as f32 / cell_h as f32).ceil()
                                } else {
                                    (bottom - top).round()
                                }
                                .max(1.0) as u16,
                            )
                        },
                        xoff,
                        yoff,
                        (crop_x, crop_y, crop_w, crop_h),
                        z,
                    );
                    let placement = ActivePlacement {
                        image_id,
                        z,
                        command,
                    };
                    if app.active_image.get(&idx) != Some(&placement) {
                        update_placement(
                            &mut out,
                            app.active_image.get(&idx),
                            &placement,
                            3000 + idx as u32,
                        )?;
                        app.active_image.insert(idx, placement);
                    }
                    visible_now.insert(idx);
                }
                if app.settings_open {
                    let panel = settings_rect(area);
                    if panel.width > 0 && panel.height > 0 {
                        let pw = panel.width as u32 * cell_w as u32;
                        let ph = panel.height as u32 * cell_h as u32;
                        if overlay_image != Some((pw, ph)) {
                            let pixels = vec![[24, 28, 32]; (pw * ph) as usize];
                            if let Some(png) = kitty::encode_png(&pixels, pw, ph) {
                                out.write_all(kitty::upload(3, &png).as_bytes())?;
                                overlay_image = Some((pw, ph));
                            }
                        }
                        if overlay_image.is_some() {
                            out.write_all(
                                kitty::place_crop(
                                    3,
                                    5001,
                                    panel.x,
                                    panel.y,
                                    panel.width,
                                    Some(panel.height),
                                    0,
                                    0,
                                    (0, 0, pw, ph),
                                    -1,
                                )
                                .as_bytes(),
                            )?;
                        }
                    }
                } else if overlay_image.take().is_some() {
                    out.write_all(kitty::delete_image(3).as_bytes())?;
                }
                for idx in visible_prev.difference(&visible_now) {
                    if let Some(placement) = app.active_image.remove(idx) {
                        out.write_all(
                            kitty::delete_placement(placement.image_id, 3000 + *idx as u32)
                                .as_bytes(),
                        )?;
                    }
                    out.write_all(kitty::delete_placement(1, 4000 + *idx as u32).as_bytes())?;
                }
                visible_prev = visible_now;
            }
            // Keep terminal window padding at the default background color.
            queue!(out, crossterm::style::ResetColor)?;
            if std::env::var("TERA_NOSYNC").is_err() {
                queue!(out, EndSynchronizedUpdate)?;
            }
            out.flush()?;
            prev.clone_from(&buf);
            frame_dirty = false;
        }

        acc += dt as f64;
        frames += 1;
        if acc >= 0.25 {
            app.fps = frames as f64 / acc;
            acc = 0.0;
            frames = 0;
        }
        let frame_time = Duration::from_secs_f64(1.0 / app.settings.fps() as f64);
        app.record_session();
        if app.session != saved_session {
            let since = session_dirty_since.get_or_insert(now);
            if now.duration_since(*since) >= Duration::from_millis(500) {
                match app.session.save(synthetic) {
                    Ok(()) => {
                        saved_session = app.session.clone();
                        session_dirty_since = None;
                    }
                    Err(error) => {
                        app.status = format!("could not save session: {error}");
                        frame_dirty = true;
                        session_dirty_since = Some(now);
                    }
                }
            }
        } else {
            session_dirty_since = None;
        }
        let elapsed = now.elapsed();
        if elapsed < frame_time {
            thread::sleep(frame_time - elapsed);
        }
    }

    app.record_session();
    let session_result = if app.session != saved_session {
        app.session.save(synthetic)
    } else {
        Ok(())
    };
    if app.kitty {
        let mut o = io::stdout();
        let _ = o.write_all(kitty::clear_placements().as_bytes());
        for id in app.uploaded.drain() {
            let _ = o.write_all(kitty::delete_image(id).as_bytes());
        }
        for (_, id) in app.pending_deletions.drain(..) {
            let _ = o.write_all(kitty::delete_image(id).as_bytes());
        }
        if overlay_image.is_some() {
            let _ = o.write_all(kitty::delete_image(3).as_bytes());
        }
        let _ = o.flush();
    }
    execute!(
        stdout,
        DisableMouseCapture,
        cursor::Show,
        terminal::LeaveAlternateScreen
    )?;
    terminal::disable_raw_mode()?;
    session_result?;
    Ok(())
}

fn draw_scene(buf: &mut Buffer, app: &App, area: Rect) -> Vec<(usize, f32, f32, f32, f32, usize)> {
    // start from a clean transparent canvas every frame; diffing against `prev` erases stale cells
    buf.reset();
    let w = area.width as i32;
    let h = area.height as i32;
    // transparent background: leave cells at the terminal default instead of painting an opaque backdrop

    let cx = w as f32 / 2.0;
    let baseline = (h as f32 * 0.68).min((h - 6) as f32);
    // adapt lead cover to the terminal size (cells), keeping square pixels via half blocks
    let cw_base = (w as f32 * 0.38).min(h as f32 * 1.1).max(4.0);

    let mut placements = Vec::new();
    let mode_name = if app.mode == ViewMode::Grid {
        "library"
    } else {
        "album flow"
    };
    let top = format!("tera   {mode_name}");
    buf.set_string(2, 1, &top, Style::default().fg(Color::Rgb(120, 130, 160)));
    if app.albums.is_empty() {
        let msg = if app.status.is_empty() {
            "loading music library…"
        } else {
            &app.status
        };
        let x = (cx as i32 - UnicodeWidthStr::width(msg) as i32 / 2).max(1);
        buf.set_string(
            x as u16,
            (h / 2) as u16,
            msg,
            Style::default().fg(Color::Rgb(200, 205, 220)),
        );
        return placements;
    }

    if app.flip > 0.0 {
        return draw_detail(buf, app, area);
    }
    if app.mode == ViewMode::Grid {
        return draw_grid(buf, app, area);
    }

    let first = (app.pos - 7.0).ceil().max(0.0) as usize;
    let last = ((app.pos + 7.0).floor() as usize).min(app.albums.len() - 1);
    let mut order: Vec<usize> = if first <= last {
        (first..=last).collect()
    } else {
        Vec::new()
    };
    order.sort_by(|&a, &b| {
        let da = (a as f32 - app.pos).abs();
        let db = (b as f32 - app.pos).abs();
        db.partial_cmp(&da).unwrap()
    });

    let (pixel_w, pixel_h) = cell_pixels();
    let aspect = (w as f32 * pixel_w as f32) / (h.max(1) as f32 * pixel_h as f32);
    let spread = flow_spread(app.settings.density, aspect);
    for i in order {
        let d = i as f32 - app.pos;
        if d.abs() > 7.0 {
            continue;
        }
        let depth = d.abs();
        let scale = 1.0 - 0.13 * flow_focus(d);
        let cw = cw_base * scale;
        let ch = cw * pixel_w as f32 / pixel_h as f32;
        let offset = if depth <= 1.0 {
            depth * 0.64
        } else {
            0.64 + (depth - 1.0) * spread
        };
        let x0 = cx + d.signum() * offset * cw_base - cw * 0.5;
        let lead_ch = cw_base * pixel_w as f32 / pixel_h as f32;
        let y0 = baseline - ch - (lead_ch - ch) * 0.5;
        let dim = 1.0;
        let bin = sprite_bin(d);
        placements.push((i, x0, y0, cw, ch, bin));
        if !app.kitty {
            let cover = app
                .covers
                .get(i)
                .and_then(|c| c.as_ref())
                .unwrap_or(&app.fallback[i % app.fallback.len()]);
            draw_cover(
                buf,
                cover,
                x0,
                y0,
                cw,
                ch,
                d,
                dim,
                w,
                h,
                app.settings.reflection,
            );
        }
    }

    let sel = &app.albums[app.selected];
    let title = &sel.album;
    let sub = format!(
        "{}   {}   {} tracks",
        if app.playing { "playing" } else { "paused" },
        sel.artist,
        sel.count
    );
    let tx = (cx as i32 - UnicodeWidthStr::width(title.as_str()) as i32 / 2).max(1);
    buf.set_string(
        tx as u16,
        (h - 6).max(0) as u16,
        title,
        Style::default().fg(Color::Rgb(240, 240, 245)),
    );
    let sx = (cx as i32 - UnicodeWidthStr::width(sub.as_str()) as i32 / 2).max(1);
    buf.set_string(
        sx as u16,
        (h - 5).max(0) as u16,
        &sub,
        Style::default().fg(Color::Rgb(130, 140, 165)),
    );

    let help = "tab grid   ←/→ move   enter album   s settings";
    let hx = (w - help.len() as i32 - 2).max(1);
    buf.set_string(
        hx as u16,
        1,
        help,
        Style::default().fg(Color::Rgb(90, 100, 125)),
    );
    if !app.status.is_empty() {
        buf.set_string(
            2,
            (h - 2).max(0) as u16,
            &app.status,
            Style::default().fg(Color::Rgb(110, 120, 145)),
        );
    }
    placements
}

fn change_setting(app: &mut App, direction: i32) {
    match app.settings_row {
        0 => {
            if app.forced_blocks {
                app.forced_blocks = false;
                app.settings.hd = true;
            } else {
                app.settings.hd = !app.settings.hd;
            }
        }
        1 => {
            app.settings.effect = (app.settings.effect as i32 + direction).rem_euclid(5) as u8;
            if app.settings.effect == 4 {
                if let Some(change) = app.wall_change.take() {
                    if let Some(other) = change.swap_slot {
                        app.wall.swap(change.slot, other);
                    } else {
                        app.wall[change.slot] = change.next;
                    }
                }
                app.wall_pending = None;
                if let Some(&idx) = app.wall.get(app.wall_slot) {
                    app.selected = idx;
                }
            }
        }
        2 => {
            app.settings.interval = (app.settings.interval as i32 + direction).rem_euclid(4) as u8;
            app.wall_next = Instant::now() + Duration::from_secs_f32(app.settings.seconds());
        }
        3 => app.settings.album_flip = !app.settings.album_flip,
        4 => app.settings.reflection = !app.settings.reflection,
        5 => app.settings.density = (app.settings.density as i32 + direction).rem_euclid(3) as u8,
        6 => {
            app.settings.frame_rate =
                (app.settings.frame_rate as i32 + direction).rem_euclid(3) as u8
        }
        7 => app.settings.scroll = (app.settings.scroll as i32 + direction).rem_euclid(5) as u8,
        _ => return,
    }
    match app.settings.save() {
        Err(error) => app.status = format!("could not save settings: {error}"),
        Ok(()) if app.status.starts_with("could not save settings:") => app.status.clear(),
        Ok(()) => {}
    }
}

fn settings_rect(area: Rect) -> Rect {
    let width = area.width.min(64);
    let height = area.height.min(16);
    Rect::new(
        (area.width - width) / 2,
        (area.height - height) / 2,
        width,
        height,
    )
}

fn draw_settings(buf: &mut Buffer, app: &App, area: Rect) {
    let panel = settings_rect(area);
    if panel.width < 4 || panel.height < 4 {
        return;
    }
    let bg = Color::Rgb(24, 28, 32);
    for y in panel.y..panel.bottom() {
        for x in panel.x..panel.right() {
            buf[(x, y)]
                .set_symbol(" ")
                .set_style(Style::default().bg(bg));
        }
    }
    let border = Style::default().fg(Color::Rgb(255, 164, 164)).bg(bg);
    let x0 = panel.x;
    let x1 = panel.right() - 1;
    let y0 = panel.y;
    let y1 = panel.bottom() - 1;
    for x in x0 + 1..x1 {
        buf.set_string(x, y0, "─", border);
        buf.set_string(x, y1, "─", border);
    }
    for y in y0 + 1..y1 {
        buf.set_string(x0, y, "│", border);
        buf.set_string(x1, y, "│", border);
    }
    for (x, y, corner) in [(x0, y0, "╭"), (x1, y0, "╮"), (x0, y1, "╰"), (x1, y1, "╯")] {
        buf.set_string(x, y, corner, border);
    }
    let title = truncate_width("┐ settings ┌", panel.width.saturating_sub(4) as usize);
    buf.set_string(x0 + 2, y0, title, border);
    let renderer = if app.forced_blocks {
        "blocks (--no-kitty)"
    } else if app.settings.hd {
        if kitty::supported() {
            "hd (kitty)"
        } else {
            "hd unavailable; using blocks"
        }
    } else {
        "blocks"
    };
    let values = [
        ("artwork", renderer.to_string()),
        ("wall transition", app.settings.effect_name().to_string()),
        ("wall interval", format!("{} s", app.settings.seconds())),
        (
            "album detail flip",
            if app.settings.album_flip {
                "on".to_string()
            } else {
                "off".to_string()
            },
        ),
        (
            "cover flow reflection",
            if app.settings.reflection {
                "on".to_string()
            } else {
                "off".to_string()
            },
        ),
        (
            "cover flow density",
            app.settings.density_name().to_string(),
        ),
        ("frame rate", format!("{} fps", app.settings.fps())),
        ("scroll sensitivity", app.settings.scroll_name().to_string()),
    ];
    for (row, (label, value)) in values.iter().enumerate() {
        let y = y0.saturating_add(3 + row as u16);
        if y >= y1 {
            break;
        }
        let text = format!(
            "{}  {:<20}  ◀ {} ▶",
            if row == app.settings_row { "›" } else { " " },
            label,
            value
        );
        let color = if row == app.settings_row {
            Color::Rgb(255, 164, 164)
        } else {
            Color::Rgb(252, 249, 234)
        };
        buf.set_string(
            x0 + 2,
            y,
            truncate_width(&text, panel.width.saturating_sub(4) as usize),
            Style::default().fg(color).bg(bg),
        );
    }
    if panel.height >= 14 {
        buf.set_string(
            x0 + 2,
            y1 - 1,
            truncate_width(
                if app.status.starts_with("could not save settings:") {
                    &app.status
                } else {
                    "↑↓ select  ←→ change  esc close"
                },
                panel.width.saturating_sub(4) as usize,
            ),
            Style::default().fg(Color::Rgb(186, 223, 219)).bg(bg),
        );
    }
}

fn flow_spread(density: u8, aspect: f32) -> f32 {
    let base = [0.38, 0.30, 0.21][density as usize];
    (base * aspect / 1.875).clamp(base, 0.65)
}

fn placement_z(index: usize, position: f32, mode: ViewMode, flip: f32, settings_open: bool) -> i32 {
    if settings_open {
        if mode == ViewMode::Flow && flip == 0.0 {
            flow_z(index, position) - 20_000
        } else {
            -2
        }
    } else if flip > 0.0 || mode == ViewMode::Grid {
        0
    } else {
        flow_z(index, position)
    }
}

fn flow_z(index: usize, position: f32) -> i32 {
    10_000 - ((index as f32 - position).abs() * 2.0).round() as i32
}

fn detail_rect(area: Rect) -> Rect {
    let width = area.width.saturating_sub(4).min(96);
    let height = area.height.saturating_sub(4).min(34);
    Rect::new(
        (area.width - width) / 2,
        (area.height - height) / 2,
        width,
        height,
    )
}

fn detail_list_top(panel: Rect) -> u16 {
    panel
        .y
        .saturating_add(10)
        .min(panel.bottom().saturating_sub(1))
}
fn detail_list_capacity(panel: Rect) -> usize {
    panel
        .bottom()
        .saturating_sub(2)
        .saturating_sub(detail_list_top(panel)) as usize
}

fn draw_detail(buf: &mut Buffer, app: &App, area: Rect) -> Vec<(usize, f32, f32, f32, f32, usize)> {
    if app.albums.is_empty() {
        return Vec::new();
    }
    let panel = detail_rect(area);
    let cx = area.width as f32 * 0.5;
    let (pixel_w, pixel_h) = cell_pixels();
    let ratio = pixel_w as f32 / pixel_h as f32;
    if app.flip < 0.5 {
        let full_width = (area.width as f32 * 0.38)
            .min(area.height as f32 * 1.1)
            .max(4.0);
        let width = (full_width * (1.0 - app.flip * 2.0)).max(0.1);
        let height = full_width * ratio;
        let x = cx - width * 0.5;
        let y = (area.height as f32 * 0.68 - height).max(0.0);
        if !app.kitty {
            let cover = app.covers[app.selected]
                .as_ref()
                .unwrap_or(&app.fallback[app.selected % app.fallback.len()]);
            draw_cover(
                buf,
                cover,
                x,
                y,
                width,
                height,
                0.0,
                1.0,
                area.width as i32,
                area.height as i32,
                false,
            );
        }
        return vec![(app.selected, x, y, width, height, 4)];
    }
    let factor = ((app.flip - 0.5) * 2.0).clamp(0.0, 1.0);
    let width = ((panel.width as f32 * factor).round() as u16)
        .max(2)
        .min(panel.width);
    if width < 3 || panel.height < 3 {
        return Vec::new();
    }
    let x0 = panel.x + (panel.width - width) / 2;
    let x1 = x0 + width - 1;
    let y0 = panel.y;
    let y1 = panel.bottom() - 1;
    let border = Style::default().fg(Color::Rgb(255, 164, 164));
    for x in x0 + 1..x1 {
        buf.set_string(x, y0, "─", border);
        buf.set_string(x, y1, "─", border);
    }
    for y in y0 + 1..y1 {
        buf.set_string(x0, y, "│", border);
        buf.set_string(x1, y, "│", border);
    }
    for (x, y, symbol) in [(x0, y0, "╭"), (x1, y0, "╮"), (x0, y1, "╰"), (x1, y1, "╯")] {
        buf.set_string(x, y, symbol, border);
    }
    if factor < 0.95 || panel.width < 20 || panel.height < 12 {
        return Vec::new();
    }
    buf.set_string(x0 + 2, y0, "┐ album ┌", border);
    let cover_w = 12.0_f32.min((panel.width.saturating_sub(6)) as f32);
    let cover_h = (cover_w * ratio).min(7.0);
    let cover_x = (panel.x + 2) as f32;
    let cover_y = (panel.y + 2) as f32;
    let info_x = panel.x.saturating_add(16);
    let info_width = panel.right().saturating_sub(info_x + 2) as usize;
    let album = &app.albums[app.selected];
    if info_width > 0 {
        buf.set_string(
            info_x,
            panel.y + 2,
            truncate_width(&album.album, info_width),
            Style::default().fg(Color::Rgb(252, 249, 234)),
        );
        buf.set_string(
            info_x,
            panel.y + 3,
            truncate_width(&album.artist, info_width),
            Style::default().fg(Color::Rgb(186, 223, 219)),
        );
        buf.set_string(
            info_x,
            panel.y + 5,
            truncate_width(&format!("{} tracks", album.count), info_width),
            Style::default().fg(Color::Rgb(186, 223, 219)),
        );
    }
    let start = detail_list_top(panel);
    let capacity = detail_list_capacity(panel);
    if !app.track_status.is_empty() && capacity > 0 {
        buf.set_string(
            panel.x + 2,
            start,
            truncate_width(&app.track_status, panel.width.saturating_sub(4) as usize),
            Style::default().fg(Color::Rgb(186, 223, 219)),
        );
    }
    let first = app
        .track_selected
        .saturating_sub(capacity.saturating_sub(1));
    for (line, track) in app.tracks.iter().skip(first).take(capacity).enumerate() {
        let index = first + line;
        let seconds = track.duration.max(0.0) as u64;
        let duration = format!("{:02}:{:02}", seconds / 60, seconds % 60);
        let available = panel.width.saturating_sub(15) as usize;
        let label = truncate_width(&track.name, available);
        let text = format!(
            "{} {:02}  {}",
            if index == app.track_selected {
                "▶"
            } else {
                " "
            },
            track.number,
            label
        );
        let style = if index == app.track_selected {
            Style::default().fg(Color::Rgb(255, 164, 164))
        } else {
            Style::default().fg(Color::Rgb(252, 249, 234))
        };
        buf.set_string(panel.x + 2, start + line as u16, text, style);
        if panel.width >= 20 {
            buf.set_string(panel.right() - 8, start + line as u16, duration, style);
        }
    }
    let footer = truncate_width(
        "↑↓ select  enter play  esc back",
        panel.width.saturating_sub(4) as usize,
    );
    buf.set_string(
        panel.x + 2,
        panel.bottom() - 2,
        footer,
        Style::default().fg(Color::Rgb(186, 223, 219)),
    );
    if !app.kitty {
        let cover = app.covers[app.selected]
            .as_ref()
            .unwrap_or(&app.fallback[app.selected % app.fallback.len()]);
        draw_cover(
            buf,
            cover,
            cover_x,
            cover_y,
            cover_w,
            cover_h,
            0.0,
            1.0,
            area.width as i32,
            area.height as i32,
            false,
        );
    }
    vec![(app.selected, cover_x, cover_y, cover_w, cover_h, 4)]
}

fn grid_metrics(area: Rect) -> (usize, usize, f32, f32, f32) {
    let w = area.width as f32;
    let h = area.height as f32;
    let cols = ((w / 24.0).round() as usize).clamp(1, 8);
    // Kitty placements use whole cell dimensions. Align tile boundaries to
    // those cells so rounding cannot make one cover overlap its neighbours.
    let cell_w = (w / cols as f32).floor().max(1.0);
    let (pixel_w, pixel_h) = cell_pixels();
    let cell_h = (cell_w * pixel_w as f32 / pixel_h as f32).round().max(1.0);
    let rows = ((h / cell_h).ceil() as usize).max(1);
    (cols, rows, cell_w, cell_h, cell_h)
}

fn grid_left(area: Rect, cols: usize, cell_w: f32) -> f32 {
    ((area.width as f32 - cols as f32 * cell_w) * 0.5)
        .floor()
        .max(0.0)
}

fn draw_grid(buf: &mut Buffer, app: &App, area: Rect) -> Vec<(usize, f32, f32, f32, f32, usize)> {
    let w = area.width as i32;
    let h = area.height as i32;
    let (cols, rows, cell_w, cell_h, _) = grid_metrics(area);
    let mut placements = Vec::new();
    for slot in 0..app.wall.len().min(cols * rows) {
        let column = slot / rows;
        let row = slot % rows;
        let mut idx = app.wall[slot];
        let x = grid_left(area, cols, cell_w) + column as f32 * cell_w;
        let y = row as f32 * cell_h;
        let (mut draw_x, mut draw_y, mut draw_w, mut draw_h) = (x, y, cell_w, cell_h);
        if let Some(change) = app
            .wall_change
            .as_ref()
            .filter(|change| change.slot == slot || change.swap_slot == Some(slot))
        {
            let t = (change.started.elapsed().as_secs_f32() / 0.8).clamp(0.0, 1.0);
            if t >= 0.5 {
                idx = if change.slot == slot {
                    change.next
                } else {
                    app.wall[change.slot]
                };
            }
            let progress = ((t - 0.5).abs() * 2.0).max(0.02);
            let eased = progress * progress * (3.0 - 2.0 * progress);
            match change.effect {
                0 => {
                    draw_w = (cell_w * eased).round().clamp(1.0, cell_w);
                    draw_x += ((cell_w - draw_w) * 0.5).floor();
                }
                1 => {
                    draw_w = (cell_w * eased).round().clamp(1.0, cell_w);
                    draw_h = (cell_h * eased).round().clamp(1.0, cell_h);
                    draw_x += ((cell_w - draw_w) * 0.5).floor();
                    draw_y += ((cell_h - draw_h) * 0.5).floor();
                }
                _ => {
                    draw_h = (cell_h * eased).round().clamp(1.0, cell_h);
                    draw_y += ((cell_h - draw_h) * 0.5).floor();
                }
            }
        }
        placements.push((idx, draw_x, draw_y, draw_w, draw_h, 2));
        if !app.kitty {
            let cover = app
                .covers
                .get(idx)
                .and_then(|v| v.as_ref())
                .unwrap_or(&app.fallback[idx % app.fallback.len()]);
            draw_cover(
                buf, cover, draw_x, draw_y, draw_w, draw_h, 0.0, 1.0, w, h, false,
            );
        }
    }

    let help = "tab flow   click album   s settings";
    let hx = (w - help.len() as i32 - 2).max(1);
    buf.set_string(
        hx as u16,
        1,
        help,
        Style::default().fg(Color::Rgb(90, 100, 125)),
    );
    let sel = &app.albums[app.selected];
    let line = truncate_width(
        &format!("{}  —  {}", sel.album, sel.artist),
        (w - 4).max(8) as usize,
    );
    buf.set_string(
        2,
        (h - 2).max(0) as u16,
        &line,
        Style::default().fg(Color::Rgb(180, 188, 205)),
    );
    placements
}

fn truncate_width(s: &str, max: usize) -> String {
    let mut out = String::new();
    let mut w = 0usize;
    for ch in s.chars() {
        let cw = UnicodeWidthStr::width(ch.to_string().as_str());
        if w + cw > max {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn draw_cover(
    buf: &mut Buffer,
    cover: &Cover,
    x0: f32,
    y0: f32,
    cw: f32,
    ch: f32,
    d: f32,
    dim: f32,
    sw: i32,
    sh: i32,
    reflect: bool,
) {
    let yaw = flow_yaw(d);
    let c = yaw.cos();
    let sn = yaw.sin();
    let k = 0.23;
    for cy in 0..ch.ceil() as i32 {
        for cx in 0..cw.ceil() as i32 {
            let gx = (x0 + cx as f32).floor() as i32;
            let gy = (y0 + cy as f32).floor() as i32;
            if gx < 0 || gy < 0 || gx >= sw || gy >= sh {
                continue;
            }
            let u = ((cx as f32 + 0.5) / cw) * 2.0 - 1.0;
            let v = ((cy as f32 + 0.5) / ch) * 2.0 - 1.0;
            let denom = c - u * sn * k;
            if denom.abs() < 0.12 {
                continue;
            }
            let x = u / denom;
            if x.abs() > 1.0 {
                continue;
            }
            let depth = 1.0 + x * sn * k;
            let y = v * (1.0 / (1.0 - sn.abs() * k)) * depth;
            if y.abs() > 1.0 {
                continue;
            }
            let su = (x + 1.0) * 0.5;
            let sv = (y + 1.0) * 0.5;
            let sv2 = (sv + 1.0 / (ch * 2.0)).min(0.999);
            let top = sample(cover, su, sv);
            let bot = sample(cover, su, sv2);
            // light from the left; receding side gets darker
            let light = dim;
            let top = shade(top, light);
            let bot = shade(bot, light);
            let cell = &mut buf[(gx as u16, gy as u16)];
            cell.set_symbol("▀");
            cell.set_style(
                Style::default()
                    .fg(Color::Rgb(top[0], top[1], top[2]))
                    .bg(Color::Rgb(bot[0], bot[1], bot[2])),
            );
        }
    }

    if reflect {
        let refl = (ch * 0.42).max(1.0) as i32;
        let ybase = (y0 + ch).ceil();
        for ry in 0..refl {
            for cx in 0..cw.ceil() as i32 {
                let gx = (x0 + cx as f32).floor() as i32;
                let gy = (ybase + ry as f32) as i32;
                if gx < 0 || gy < 0 || gx >= sw || gy >= sh {
                    continue;
                }
                let u = ((cx as f32 + 0.5) / cw) * 2.0 - 1.0;
                let v = 1.0 - ((ry as f32 + 0.5) / ch) * 2.0;
                let denom = c - u * sn * k;
                if denom.abs() < 0.12 {
                    continue;
                }
                let x = u / denom;
                if x.abs() > 1.0 {
                    continue;
                }
                let y = v * (1.0 / (1.0 - sn.abs() * k)) * (1.0 + x * sn * k);
                if y.abs() > 1.0 {
                    continue;
                }
                let su = (x + 1.0) * 0.5;
                let sv = (y + 1.0) * 0.5;
                // A terminal cell has no alpha channel. Ordered dithering leaves
                // unpainted half-cells at the terminal default, instead of black.
                let opacity = (1.0 - ry as f32 / refl as f32).powf(1.8) * 0.35 * dim;
                let threshold = |half_y: i32| {
                    const BAYER: [[u8; 4]; 4] =
                        [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
                    (BAYER[(half_y as usize) % 4][(gx as usize) % 4] as f32 + 0.5) / 16.0
                };
                let show_top = opacity > threshold(gy * 2);
                let show_bot = opacity > threshold(gy * 2 + 1);
                if !show_top && !show_bot {
                    continue;
                }
                let top = shade(sample(cover, su, sv), 0.7);
                let bot = shade(sample(cover, su, (sv - 1.0 / (ch * 2.0)).max(0.0)), 0.7);
                let cell = &mut buf[(gx as u16, gy as u16)];
                let (symbol, fg, bg) = match (show_top, show_bot) {
                    (true, true) => (
                        "▀",
                        Color::Rgb(top[0], top[1], top[2]),
                        Color::Rgb(bot[0], bot[1], bot[2]),
                    ),
                    (true, false) => ("▀", Color::Rgb(top[0], top[1], top[2]), Color::Reset),
                    (false, true) => ("▄", Color::Rgb(bot[0], bot[1], bot[2]), Color::Reset),
                    _ => unreachable!(),
                };
                cell.set_symbol(symbol);
                cell.set_style(Style::default().fg(fg).bg(bg));
            }
        }
    }
}

fn sample(cover: &Cover, u: f32, v: f32) -> [u8; 3] {
    let x = (u * (cover.w - 1) as f32) as usize;
    let y = (v * (cover.h - 1) as f32) as usize;
    cover.px[y.min(cover.h - 1) * cover.w + x.min(cover.w - 1)]
}

fn shade(c: [u8; 3], f: f32) -> [u8; 3] {
    [
        (c[0] as f32 * f) as u8,
        (c[1] as f32 * f) as u8,
        (c[2] as f32 * f) as u8,
    ]
}

struct CountWriter<W: Write> {
    inner: W,
    bytes: usize,
}

impl<W: Write> Write for CountWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.bytes += buf.len();
        self.inner.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

fn bench(frames: usize) -> Result<()> {
    let mut app = App::synthetic();
    app.kitty = false;
    if let Some(count) = std::env::var("TERA_BENCH_ALBUMS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&n| n > 0)
    {
        let albums = app.albums.clone();
        app.albums = (0..count)
            .map(|i| albums[i % albums.len()].clone())
            .collect();
        app.fallback = (0..count.min(100)).map(fallback_cover).collect();
        app.covers = (0..count).map(|_| None).collect();
    }
    let area = Rect::new(0, 0, 200, 60);
    let mut buf = Buffer::empty(area);
    let mut prev = Buffer::empty(area);
    let mut total_bytes = 0usize;
    let start = Instant::now();
    for f in 0..frames {
        let t = f as f32 / 120.0;
        app.pos = (app.albums.len() / 2) as f32 + 3.5 * (t * 2.0).sin();
        app.selected = app.pos.round().clamp(0.0, (app.albums.len() - 1) as f32) as usize;
        app.fps = 120.0;
        draw_scene(&mut buf, &app, area);
        let mut sink = CountWriter {
            inner: Vec::with_capacity(1 << 20),
            bytes: 0,
        };
        encode_diff(&mut sink, &prev, &buf);
        total_bytes += sink.bytes;
        prev.clone_from(&buf);
    }
    let dt = start.elapsed().as_secs_f64();
    println!(
        "bench: {} frames in {:.3}s = {:.1} fps ({:.2} ms/frame, avg {:.1} kb/frame, {:.2} mb/s @120fps)",
        frames,
        dt,
        frames as f64 / dt,
        dt * 1000.0 / frames as f64,
        total_bytes as f64 / frames as f64 / 1024.0,
        total_bytes as f64 / frames as f64 * 120.0 / (1024.0 * 1024.0)
    );
    Ok(())
}

fn encode_diff<W: Write>(out: &mut W, prev: &Buffer, next: &Buffer) -> usize {
    let mut last: Option<(u16, u16)> = None;
    let mut last_fg: Option<Color> = None;
    let mut last_bg: Option<Color> = None;
    let mut written = 0usize;
    for (x, y, cell) in prev.diff(next) {
        let sym = cell.symbol();
        if sym.is_empty() {
            // continuation cell of a wide glyph: the glyph itself already covered it
            continue;
        }
        let width = UnicodeWidthStr::width(sym).max(1) as u16;
        if last != Some((x, y)) {
            write!(out, "\x1b[{};{}H", y + 1, x + 1).unwrap();
        }
        let fg = cell.fg;
        let bg = cell.bg;
        if last_fg != Some(fg) || last_bg != Some(bg) {
            let fg_s = match fg {
                Color::Reset => "39".to_string(),
                _ => {
                    let (r, g, b) = rgb(fg);
                    format!("38;2;{r};{g};{b}")
                }
            };
            let bg_s = match bg {
                Color::Reset => "49".to_string(),
                _ => {
                    let (r, g, b) = rgb(bg);
                    format!("48;2;{r};{g};{b}")
                }
            };
            write!(out, "\x1b[{fg_s};{bg_s}m").unwrap();
            last_fg = Some(fg);
            last_bg = Some(bg);
        }
        write!(out, "{sym}").unwrap();
        written += 1;
        last = Some((x + width, y));
    }
    written
}

fn rgb(c: Color) -> (u8, u8, u8) {
    match c {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Black => (0, 0, 0),
        Color::White => (255, 255, 255),
        _ => (0, 0, 0),
    }
}

#[cfg(test)]
mod layout_tests {
    use super::*;

    #[test]
    fn repeated_keys_do_not_queue_far_ahead_and_can_reverse() {
        let mut app = App::synthetic();
        for _ in 0..30 {
            app.move_flow(1);
        }
        assert_eq!(app.selected, 1);
        app.pos = 0.4;
        for _ in 0..30 {
            app.move_flow(1);
        }
        assert_eq!(app.selected, 2);
        app.move_flow(-1);
        assert_eq!(app.selected, 1);
    }

    #[test]
    fn session_restores_album_after_library_reordering() {
        let mut app = App::synthetic();
        app.selected = 5;
        app.record_session();
        let expected = app.albums[5].album.clone();
        app.albums.reverse();
        app.selected = 0;
        app.restore_selection(None);
        assert_eq!(app.albums[app.selected].album, expected);
        assert_eq!(app.pos, app.selected as f32);
    }

    #[test]
    fn grid_session_preserves_view_without_restoring_a_wall_position() {
        let mut app = App::synthetic();
        app.selected = 4;
        app.record_session();
        app.mode = ViewMode::Grid;
        app.record_session();
        assert!(app.session.grid);
        app.selected = 0;
        app.restore_selection(None);
        assert_eq!(app.selected, 0);
        assert_eq!(app.session.album.as_deref(), Some("Velvet Static"));
        app.restore_selection(Some(7));
        assert_eq!(app.selected, 7);
    }

    #[test]
    fn restores_old_session_saved_with_a_track_artist() {
        let mut app = App::synthetic();
        app.albums[4].artist = "Album Artist".into();
        app.albums[4].track_artists = vec!["Singer A".into(), "Singer B".into()];
        app.session.album = Some(app.albums[4].album.clone());
        app.session.artist = Some("Singer B".into());
        app.restore_selection(None);
        assert_eq!(app.selected, 4);
        app.record_session();
        assert_eq!(app.session.artist.as_deref(), Some("Album Artist"));
    }

    #[test]
    fn missing_saved_album_falls_back_to_first() {
        let mut app = App::synthetic();
        app.session.album = Some("Removed album".into());
        app.session.artist = Some("Unknown".into());
        app.selected = 7;
        app.restore_selection(None);
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn block_cover_extent_does_not_reverse_while_returning_to_center() {
        let area = Rect::new(0, 0, 120, 40);
        let cover = fallback_cover(0);
        let mut previous_width = 0;
        for step in (0..=100).rev() {
            let distance = step as f32 / 100.0;
            let width = 44.0 * (1.0 - 0.13 * flow_focus(distance));
            let height = width * 0.5;
            let x = 60.0 + distance * 0.64 * 44.0 - width * 0.5;
            let mut buffer = Buffer::empty(area);
            draw_cover(
                &mut buffer,
                &cover,
                x,
                5.0,
                width,
                height,
                distance,
                1.0,
                120,
                40,
                false,
            );
            let mut left = area.width;
            let mut right = 0;
            for y in 0..area.height {
                for x in 0..area.width {
                    if buffer[(x, y)].symbol() != " " {
                        left = left.min(x);
                        right = right.max(x);
                    }
                }
            }
            let visible_width = right.saturating_sub(left) + 1;
            assert!(
                visible_width + 1 >= previous_width,
                "width shrank from {previous_width} to {visible_width} at {distance}"
            );
            previous_width = visible_width;
        }
    }

    #[test]
    fn subcell_offsets_do_not_squeeze_the_visible_cover() {
        for width_step in 0..=120 {
            let desired = 300.0 + width_step as f32 * 0.4;
            for xoff in 0..8 {
                let cols = ((desired + xoff as f32) / 8.0).ceil();
                let available = cols * 8.0 - xoff as f32;
                let crop = flow_crop_width(384, 384, 0, desired, available);
                let visible_pixels = available * 384.0 / crop as f32;
                assert!(
                    (visible_pixels - desired).abs() < 0.6,
                    "desired {desired}, got {visible_pixels}, X {xoff}"
                );
            }
        }
    }

    #[test]
    fn returning_cover_grows_smoothly_without_reversing() {
        let mut previous_scale = 0.0;
        let mut previous_yaw = f32::INFINITY;
        for step in (0..=100).rev() {
            let distance = step as f32 / 100.0;
            let scale = 1.0 - 0.13 * flow_focus(distance);
            let yaw = flow_yaw(distance).abs();
            assert!(scale >= previous_scale);
            assert!(yaw <= previous_yaw);
            assert!((flow_yaw(distance) + flow_yaw(-distance)).abs() < 1e-6);
            previous_scale = scale;
            previous_yaw = yaw;
        }
        assert!(flow_yaw(0.01).abs() < 0.001);
    }

    #[test]
    fn block_reflection_does_not_change_the_cover_cells() {
        let area = Rect::new(0, 0, 80, 40);
        let cover = fallback_cover(0);
        for &(y, height, distance) in &[(4.3, 16.5, 0.0), (4.8, 16.2, 0.3), (5.2, 15.7, 1.0)] {
            let mut plain = Buffer::empty(area);
            let mut reflected = Buffer::empty(area);
            draw_cover(
                &mut plain, &cover, 8.2, y, 32.5, height, distance, 1.0, 80, 40, false,
            );
            draw_cover(
                &mut reflected,
                &cover,
                8.2,
                y,
                32.5,
                height,
                distance,
                1.0,
                80,
                40,
                true,
            );
            for row in 0..(y + height).ceil() as u16 {
                for x in 0..area.width {
                    assert_eq!(
                        plain[(x, row)],
                        reflected[(x, row)],
                        "reflection changed cover at ({x}, {row})"
                    );
                }
            }
        }
    }

    #[test]
    fn flow_image_height_is_derived_from_width_with_or_without_reflection() {
        let cover = kitty::place_crop(6001, 3001, 0, 0, 30, None, 0, 0, (0, 0, 384, 384), 1);
        let reflected = kitty::place_crop(6001, 3001, 0, 0, 30, None, 0, 0, (0, 0, 384, 545), 1);
        assert!(cover.contains(",c=30,X="));
        assert!(reflected.contains(",c=30,X="));
        assert!(!cover.contains(",r="));
        assert!(!reflected.contains(",r="));
    }

    #[test]
    fn block_reflection_keeps_transparent_cells() {
        let area = Rect::new(0, 0, 40, 30);
        let mut buf = Buffer::empty(area);
        let cover = Cover {
            px: Arc::new(vec![[255, 255, 255]; 64 * 64]),
            w: 64,
            h: 64,
        };
        draw_cover(
            &mut buf, &cover, 5.0, 2.0, 24.0, 12.0, 0.0, 1.0, 40, 30, true,
        );
        let reflection = (14..19)
            .flat_map(|y| (5..29).map(move |x| (x, y)))
            .map(|xy| &buf[xy])
            .collect::<Vec<_>>();
        assert!(reflection.iter().any(|cell| cell.symbol() != " "));
        assert!(
            reflection
                .iter()
                .any(|cell| cell.symbol() == " " && cell.bg == Color::Reset)
        );
        assert!(
            reflection
                .iter()
                .any(|cell| cell.symbol() != " " && cell.bg == Color::Reset)
        );
    }

    #[test]
    fn wall_opens_details_without_flow_flip() {
        let mut app = App::synthetic();
        app.mode = ViewMode::Grid;
        app.open_detail();
        assert_eq!(app.flip, 1.0);
        assert!(app.detail_open);
    }

    #[test]
    fn density_options_change_cover_spacing() {
        assert!(flow_spread(0, 2.0) > flow_spread(1, 2.0));
        assert!(flow_spread(1, 2.0) > flow_spread(2, 2.0));
    }

    #[test]
    fn stale_art_request_is_requeued_if_the_user_returns() {
        let mut app = App::synthetic();
        app.albums.resize(101, app.albums[0].clone());
        app.covers.resize_with(101, || None);
        app.pngs.resize_with(101, || None);
        app.sprites.resize_with(101, || None);
        let (requests, rx_requests) = mpsc::channel();
        let (messages, rx_messages) = mpsc::channel();
        app.tx_req = Some(requests);
        app.rx = Some(rx_messages);
        app.selected = 0;
        app.request_around();
        app.selected = 100;
        app.request_around();
        assert!(!app.wanted.lock().unwrap().contains(&0));
        app.selected = 0;
        app.request_around();
        assert!(app.wanted.lock().unwrap().contains(&0));
        messages.send(Msg::Skipped(0)).unwrap();
        app.poll();
        assert_eq!(rx_requests.try_iter().filter(|&idx| idx == 0).count(), 2);
    }

    #[test]
    fn evicting_a_cover_releases_its_terminal_images() {
        let mut app = App::synthetic();
        app.albums.resize(101, app.albums[0].clone());
        app.covers.resize_with(101, || None);
        app.pngs.resize_with(101, || None);
        app.sprites.resize_with(101, || None);
        app.covers[0] = Some(Cover {
            px: Arc::new(vec![[1, 2, 3]]),
            w: 1,
            h: 1,
        });
        app.uploaded.insert(2000);
        app.uploaded.insert(6000);
        app.selected = 100;
        app.request_around();
        assert!(app.covers[0].is_none());
        assert!(app.pending_deletions.contains(&(0, 2000)));
        assert!(app.pending_deletions.contains(&(0, 6000)));
        assert!(!app.uploaded.contains(&2000));
        assert!(!app.uploaded.contains(&6000));
    }

    #[test]
    fn flow_requests_visible_angles_first_then_prewarms_neighbours() {
        let mut app = App::synthetic();
        app.kitty = true;
        app.selected = 5;
        app.pos = 5.0;
        let (tx, rx) = mpsc::channel();
        app.tx_sprites = Some(tx);
        app.request_flow_sprites();
        let requests: Vec<_> = rx.try_iter().map(|(idx, bin, _, _)| (idx, bin)).collect();
        let visible_count = app.albums.len();
        assert_eq!(requests.len(), visible_count + 3 * (sprite::BINS - 1));
        assert!(requests[..visible_count].contains(&(5, sprite::BINS / 2)));
        assert!(
            requests[..visible_count]
                .iter()
                .all(|&(i, bin)| bin == sprite_bin(i as f32 - 5.0))
        );
        assert!(
            requests[visible_count..]
                .iter()
                .all(|&(i, _)| i.abs_diff(5) <= 1)
        );
        app.request_flow_sprites();
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn unfinished_angle_uses_closest_ready_sprite() {
        let mut bins = vec![Vec::new(); sprite::BINS];
        assert_eq!(closest_ready_sprite(&bins, 5), None);
        bins[2] = vec![1];
        bins[8] = vec![1];
        assert_eq!(closest_ready_sprite(&bins, 7), Some(8));
        assert_eq!(closest_ready_sprite(&bins, 3), Some(2));
        bins[7] = vec![1];
        assert_eq!(closest_ready_sprite(&bins, 7), Some(7));
    }

    #[test]
    fn switching_images_places_new_before_deleting_old() {
        let old = ActivePlacement {
            image_id: 6001,
            z: 10,
            command: "old".into(),
        };
        let new = ActivePlacement {
            image_id: 6002,
            z: 10,
            command: "new".into(),
        };
        let mut output = Vec::new();
        update_placement(&mut output, Some(&old), &new, 3001).unwrap();
        let output = String::from_utf8(output).unwrap();
        assert!(output.starts_with("new"));
        assert!(output.contains("a=d,d=i,i=6001,p=3001"));
    }

    #[test]
    fn floating_settings_preserve_flow_depth_under_the_panel() {
        let center = placement_z(5, 5.0, ViewMode::Flow, 0.0, true);
        let near_right = placement_z(6, 5.0, ViewMode::Flow, 0.0, true);
        let far_right = placement_z(7, 5.0, ViewMode::Flow, 0.0, true);
        let near_left = placement_z(4, 5.0, ViewMode::Flow, 0.0, true);
        let far_left = placement_z(3, 5.0, ViewMode::Flow, 0.0, true);
        assert!(center > near_right && near_right > far_right);
        assert!(center > near_left && near_left > far_left);
        assert!(center < -1 && near_right < -1 && far_right < -1);
        assert_eq!(placement_z(5, 5.0, ViewMode::Grid, 0.0, true), -2);
    }

    #[test]
    fn nearer_flow_covers_stay_above_farther_ones() {
        assert!(flow_z(5, 5.0) > flow_z(6, 5.0));
        assert!(flow_z(6, 5.0) > flow_z(7, 5.0));
        assert!(flow_z(4, 5.0) > flow_z(3, 5.0));
    }

    #[test]
    fn detail_displays_tracks_and_selection() {
        let mut app = App::synthetic();
        app.open_detail();
        app.flip = 1.0;
        app.track_selected = 1;
        app.kitty = false;
        let area = Rect::new(0, 0, 120, 40);
        let mut buf = Buffer::empty(area);
        draw_scene(&mut buf, &app, area);
        let panel = detail_rect(area);
        let row = detail_list_top(panel) + 1;
        assert_eq!(buf[(panel.x + 2, row)].symbol(), "▶");
        assert_eq!(app.tracks[app.track_selected].name, "track 02");
    }

    #[test]
    fn wall_replaces_one_tile_and_keeps_selection_consistent() {
        let mut app = App::synthetic();
        app.mode = ViewMode::Grid;
        let area = Rect::new(0, 0, 48, 20);
        app.sync_wall(area);
        let initial = app.wall.clone();
        let selected = app.selected;
        let now = Instant::now();
        app.wall_next = now;
        app.update_wall(now);
        app.update_wall(now);
        let change = app.wall_change.as_mut().expect("replacement should start");
        let slot = change.slot;
        let next = change.next;
        assert!(!initial.contains(&next));
        change.started = now - Duration::from_secs(1);
        app.update_wall(now);
        assert_eq!(app.wall[slot], next);
        assert_eq!(
            app.selected,
            if slot == app.wall_slot {
                next
            } else {
                selected
            }
        );
        assert_eq!(
            app.wall
                .iter()
                .zip(&initial)
                .filter(|(a, b)| a != b)
                .count(),
            1
        );
    }

    #[test]
    fn small_wall_swaps_both_images_without_duplicates() {
        let mut app = App::synthetic();
        app.mode = ViewMode::Grid;
        let area = Rect::new(0, 0, 120, 40);
        app.sync_wall(area);
        let now = Instant::now();
        app.wall_next = now;
        app.update_wall(now);
        let change = app.wall_change.as_mut().unwrap();
        let a = change.slot;
        let b = change.swap_slot.unwrap();
        change.started = now - Duration::from_millis(600);
        let mut buf = Buffer::empty(area);
        let placements = draw_grid(&mut buf, &app, area);
        assert_eq!(placements[a].0, app.wall[b]);
        assert_eq!(placements[b].0, app.wall[a]);
        assert_ne!(a, b);
    }

    #[test]
    fn flow_tilts_away_from_the_center() {
        let mut app = App::synthetic();
        app.selected = 5;
        app.pos = 5.0;
        let area = Rect::new(0, 0, 120, 40);
        let mut buf = Buffer::empty(area);
        let placements = draw_scene(&mut buf, &app, area);
        assert!(placements.iter().find(|p| p.0 == 4).unwrap().5 > sprite::BINS / 2);
        assert!(placements.iter().find(|p| p.0 == 6).unwrap().5 < sprite::BINS / 2);
    }

    #[test]
    fn clicking_during_a_transition_opens_the_visible_album() {
        for elapsed in [100, 600] {
            let mut app = App::synthetic();
            app.mode = ViewMode::Grid;
            let area = Rect::new(0, 0, 48, 20);
            app.sync_wall(area);
            let old = app.wall[1];
            let next = app.wall.len();
            app.wall_change = Some(WallChange {
                slot: 1,
                next,
                started: Instant::now() - Duration::from_millis(elapsed),
                effect: 0,
                swap_slot: None,
            });
            app.move_wall_selection(1);
            assert_eq!(app.selected, if elapsed < 400 { old } else { next });
        }
    }

    #[test]
    fn settings_draws_floating_without_changing_the_album() {
        let app = App::synthetic();
        let area = Rect::new(0, 0, 120, 40);
        let mut buf = Buffer::empty(area);
        draw_settings(&mut buf, &app, area);
        let floating = settings_rect(area);
        assert_eq!(buf[(floating.x, floating.y)].symbol(), "╭");
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn wall_transition_stays_inside_its_tile_at_fractional_window_sizes() {
        let area = Rect::new(0, 0, 127, 40);
        for effect in 0..3 {
            for elapsed in [0, 200, 400, 600, 800] {
                let mut app = App::synthetic();
                app.mode = ViewMode::Grid;
                app.sync_wall(area);
                let slot = 1;
                app.wall_change = Some(WallChange {
                    slot,
                    next: app.wall[slot],
                    started: Instant::now() - Duration::from_millis(elapsed),
                    effect,
                    swap_slot: None,
                });
                let (cols, rows, width, height, _) = grid_metrics(area);
                let mut buf = Buffer::empty(area);
                let (_, x, y, w, h, _) = draw_grid(&mut buf, &app, area)[slot];
                let tile_x = grid_left(area, cols, width) + (slot / rows) as f32 * width;
                let tile_y = (slot % rows) as f32 * height;
                assert!(x >= tile_x && x + w <= tile_x + width);
                assert!(y >= tile_y && y + h <= tile_y + height);
                assert_eq!(
                    (x.fract(), y.fract(), w.fract(), h.fract()),
                    (0.0, 0.0, 0.0, 0.0)
                );
            }
        }
    }

    #[test]
    fn grid_is_fixed_and_selection_moves_between_tiles() {
        let mut app = App::synthetic();
        app.mode = ViewMode::Grid;
        let area = Rect::new(0, 0, 120, 40);
        app.sync_wall(area);
        let mut buf = Buffer::empty(area);
        let (_, rows, width, height, _) = grid_metrics(area);
        let placements = draw_grid(&mut buf, &app, area);
        assert_eq!(placements[0].0, 0);
        assert_eq!(placements[0].1, 0.0);
        assert!((placements[1].2 - height).abs() < 0.001);
        assert!((placements[rows].1 - width).abs() < 0.001);
        app.move_wall_selection(rows as isize);
        assert_eq!(app.selected, app.wall[rows]);
        assert_eq!(draw_grid(&mut buf, &app, area)[0].1, 0.0);
    }
}
