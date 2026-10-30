use base64::Engine;
use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder};

pub fn supported() -> bool {
    std::env::var("TERM_PROGRAM")
        .map(|v| v == "ghostty")
        .unwrap_or(false)
        || std::env::var("TERM")
            .map(|v| v.contains("kitty"))
            .unwrap_or(false)
        || std::env::var("KITTY_WINDOW_ID").is_ok()
}

pub fn encode_png(px: &[[u8; 3]], w: u32, h: u32) -> Option<Vec<u8>> {
    let mut raw = Vec::with_capacity(px.len() * 3);
    for p in px {
        raw.extend_from_slice(p);
    }
    let mut out = Vec::new();
    PngEncoder::new(&mut out)
        .write_image(&raw, w, h, ExtendedColorType::Rgb8)
        .ok()?;
    Some(out)
}

/// Transmit a PNG and keep it in the terminal's image store.
pub fn upload(id: u32, png: &[u8]) -> String {
    let b64 = base64::engine::general_purpose::STANDARD.encode(png);
    let mut out = String::new();
    let chunks: Vec<&[u8]> = b64.as_bytes().chunks(4096).collect();
    for (i, chunk) in chunks.iter().enumerate() {
        let last = i + 1 == chunks.len();
        let m = if last { 0 } else { 1 };
        if i == 0 {
            out.push_str(&format!("\x1b_Gf=100,a=t,i={id},q=2,m={m};"));
        } else {
            out.push_str(&format!("\x1b_Gm={m};"));
        }
        out.push_str(std::str::from_utf8(chunk).unwrap());
        out.push_str("\x1b\\");
    }
    out
}

/// Delete every placed image; the transmitted image data is kept.
pub fn clear_placements() -> &'static str {
    "\x1b_Ga=d,d=a\x1b\\"
}

#[allow(clippy::too_many_arguments)]
pub fn place_crop(
    id: u32,
    pid: u32,
    col: u16,
    row: u16,
    cols: u16,
    rows: u16,
    xoff: u16,
    yoff: u16,
    crop: (u32, u32, u32, u32),
    z: i32,
) -> String {
    let (x, y, w, h) = crop;
    format!(
        "\x1b[{};{}H\x1b_Ga=p,i={id},p={pid},x={x},y={y},w={w},h={h},c={cols},r={rows},X={xoff},Y={yoff},z={z},C=1,q=2\x1b\\",
        row + 1,
        col + 1
    )
}

/// Delete one placement, keeping the transmitted image data.
pub fn delete_placement(id: u32, pid: u32) -> String {
    format!("\x1b_Ga=d,d=i,i={id},p={pid}\x1b\\")
}
