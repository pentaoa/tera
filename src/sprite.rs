use image::codecs::png::PngEncoder;
use image::{ExtendedColorType, ImageEncoder};

pub const BINS: usize = 9;
pub const SPRITE_SIZE: u32 = 384;

pub fn yaw_for_bin(bin: usize) -> f32 {
    -1.05 + bin as f32 * (2.1 / 8.0)
}

/// Warp a square cover into a perspective (Y-rotated) RGBA sprite.
pub fn warp_rgba(px: &[[u8; 3]], size: u32, yaw: f32, out: u32) -> Vec<u8> {
    let mut buf = vec![0u8; (out * out * 4) as usize];
    let c = yaw.cos();
    let s = yaw.sin();
    let k = 0.23f32;
    let n = size as usize;
    for oy in 0..out {
        for ox in 0..out {
            let u = ((ox as f32 + 0.5) / out as f32) * 2.0 - 1.0;
            let v = ((oy as f32 + 0.5) / out as f32) * 2.0 - 1.0;
            let denom = c - u * s * k;
            if denom.abs() < 0.12 {
                continue;
            }
            let x = u / denom;
            if x.abs() > 1.0 {
                continue;
            }
            // The near edge projects beyond the original square. Fit the entire
            // projected quad into the sprite instead of cutting off its corners.
            let y = v * (1.0 / (1.0 - s.abs() * k)) * (1.0 + x * s * k);
            if y.abs() > 1.0 {
                continue;
            }
            let su = (x + 1.0) * 0.5;
            let sv = (y + 1.0) * 0.5;
            let fx = su * (n - 1) as f32;
            let fy = sv * (n - 1) as f32;
            let x0 = fx.floor() as usize;
            let y0 = fy.floor() as usize;
            let x1 = (x0 + 1).min(n - 1);
            let y1 = (y0 + 1).min(n - 1);
            let tx = fx - x0 as f32;
            let ty = fy - y0 as f32;
            let p00 = px[y0 * n + x0];
            let p10 = px[y0 * n + x1];
            let p01 = px[y1 * n + x0];
            let p11 = px[y1 * n + x1];
            let mut rgb = [0u8; 3];
            for i in 0..3 {
                let a = p00[i] as f32 * (1.0 - tx) + p10[i] as f32 * tx;
                let b = p01[i] as f32 * (1.0 - tx) + p11[i] as f32 * tx;
                rgb[i] = (a * (1.0 - ty) + b * ty).clamp(0.0, 255.0) as u8;
            }
            let light = 1.0;
            let idx = ((oy * out + ox) * 4) as usize;
            buf[idx] = (rgb[0] as f32 * light) as u8;
            buf[idx + 1] = (rgb[1] as f32 * light) as u8;
            buf[idx + 2] = (rgb[2] as f32 * light) as u8;
            buf[idx + 3] = 255;
        }
    }
    buf
}

pub fn encode_rgba_wh(raw: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    PngEncoder::new(&mut out)
        .write_image(raw, w, h, ExtendedColorType::Rgba8)
        .ok()?;
    Some(out)
}

/// Bake a faded, vertically mirrored reflection underneath the card.
pub fn with_reflection(card: &[u8], size: u32, refl_h: u32) -> (Vec<u8>, u32) {
    let w = size as usize;
    let total_h = size + refl_h;
    let mut out = vec![0u8; w * total_h as usize * 4];
    out[..card.len()].copy_from_slice(card);
    for ry in 0..refl_h as usize {
        let src_y = (size as usize - 1).saturating_sub(ry);
        let fade = (1.0 - ry as f32 / refl_h as f32).powf(1.8) * 0.16;
        for x in 0..w {
            let si = (src_y * w + x) * 4;
            let di = ((size as usize + ry) * w + x) * 4;
            let a = card[si + 3] as f32 * fade;
            out[di] = (card[si] as f32 * 0.78) as u8;
            out[di + 1] = (card[si + 1] as f32 * 0.78) as u8;
            out[di + 2] = (card[si + 2] as f32 * 0.78) as u8;
            out[di + 3] = a as u8;
        }
    }
    (out, total_h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotated_cover_keeps_all_four_corners() {
        let size = 128usize;
        let mut pixels = vec![[0, 0, 0]; size * size];
        for y in 0..size {
            for x in 0..size {
                if (x < 10 || x >= size - 10) && (y < 10 || y >= size - 10) {
                    pixels[y * size + x] = [
                        if x < 10 { 255 } else { 0 },
                        if y < 10 { 255 } else { 0 },
                        255,
                    ];
                }
            }
        }
        for yaw in [-1.05, 1.05] {
            let image = warp_rgba(&pixels, size as u32, yaw, 384);
            for color in [[255, 255, 255], [0, 255, 255], [255, 0, 255], [0, 0, 255]] {
                assert!(
                    image
                        .chunks_exact(4)
                        .any(|p| p[3] == 255 && p[0..3] == color),
                    "missing corner {color:?} at yaw {yaw}"
                );
            }
        }
    }

    #[test]
    fn reflection_mirrors_adjacent_rows_without_compressing_them() {
        let mut card = vec![0; 4 * 4 * 4];
        for y in 0..4 {
            for x in 0..4 {
                let i = (y * 4 + x) * 4;
                card[i] = (y * 50) as u8;
                card[i + 3] = 255;
            }
        }
        let (image, _) = with_reflection(&card, 4, 2);
        assert_eq!(image[4 * 4 * 4], (150.0_f32 * 0.78) as u8);
        assert_eq!(image[5 * 4 * 4], (100.0_f32 * 0.78) as u8);
    }
}
