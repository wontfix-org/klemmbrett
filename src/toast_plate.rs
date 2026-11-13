//! Supersampled coverage for a toast card.
//!
//! GPUI's rounded-rect shader fades about half a device pixel, and a 1px
//! border snapped to the pixel grid still steps. Each output pixel here is
//! the fraction of an 8×8 grid that falls inside the shape, so the edge is
//! antialiased without painting a halo outside it.

use crate::Rgba;
use gpui::RenderImage;
use image::{Frame, ImageBuffer, Rgba as ImageRgba};

const SAMPLES: u32 = 8;

#[derive(Clone, Copy, PartialEq)]
pub struct Key {
    width: u32,
    height: u32,
    radius: u32,
    border: u32,
    fill: u32,
    stroke: u32,
}

impl Key {
    pub fn new(
        width: f32,
        height: f32,
        radius: f32,
        border: f32,
        fill: Rgba,
        stroke: Rgba,
        scale: f32,
    ) -> Self {
        let px = |value: f32| (value * scale).max(0.0).round() as u32;
        Self {
            width: px(width).max(1),
            height: px(height).max(1),
            radius: px(radius),
            border: px(border),
            fill: fill.to_u32(),
            stroke: stroke.to_u32(),
        }
    }
}

pub fn rasterize(key: Key) -> RenderImage {
    let width = key.width;
    let height = key.height;
    let radius = key.radius as f32;
    let border = key.border as f32;
    let fill = decode(key.fill);
    let stroke = decode(key.stroke);
    let mut bytes = vec![0u8; (width * height * 4) as usize];
    let n = SAMPLES as f32;

    for y in 0..height {
        for x in 0..width {
            let mut covered = 0u32;
            let mut linear = [0.0f32; 3];
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let px = x as f32 + (sx as f32 + 0.5) / n;
                    let py = y as f32 + (sy as f32 + 0.5) / n;
                    let Some(color) = sample(
                        px,
                        py,
                        width as f32,
                        height as f32,
                        radius,
                        border,
                        fill,
                        stroke,
                    ) else {
                        continue;
                    };
                    covered += 1;
                    let straight = [color.r, color.g, color.b];
                    for (channel, value) in linear.iter_mut().zip(straight) {
                        *channel += srgb_to_linear(value);
                    }
                }
            }
            let alpha = ((covered as f32 / (n * n)) * 255.0).round() as u8;
            let offset = ((y * width + x) * 4) as usize;
            if covered == 0 {
                continue;
            }
            let scale = 1.0 / covered as f32;
            // Straight BGRA. The atlas keeps these bytes for Bgra8Unorm and
            // swaps red and blue when the texture is Rgba8Unorm.
            bytes[offset] = linear_to_srgb(linear[2] * scale);
            bytes[offset + 1] = linear_to_srgb(linear[1] * scale);
            bytes[offset + 2] = linear_to_srgb(linear[0] * scale);
            bytes[offset + 3] = alpha;
        }
    }

    let buffer = ImageBuffer::<ImageRgba<u8>, _>::from_raw(width, height, bytes)
        .expect("plate buffer length matches its dimensions");
    RenderImage::new(vec![Frame::new(buffer)])
}

fn sample(
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    radius: f32,
    border: f32,
    fill: Rgba,
    stroke: Rgba,
) -> Option<Rgba> {
    if rounded_distance(x, y, width, height, radius) > 0.0 {
        return None;
    }
    let inner_w = width - 2.0 * border;
    let inner_h = height - 2.0 * border;
    let in_stroke = border > 0.0
        && (inner_w <= 0.0
            || inner_h <= 0.0
            || rounded_distance(
                x - border,
                y - border,
                inner_w,
                inner_h,
                (radius - border).max(0.0),
            ) > 0.0);
    Some(if in_stroke { stroke } else { fill })
}

/// Signed distance to a rounded rectangle at the origin. Negative inside.
fn rounded_distance(x: f32, y: f32, width: f32, height: f32, radius: f32) -> f32 {
    let half_w = width * 0.5;
    let half_h = height * 0.5;
    let radius = radius.min(half_w).min(half_h).max(0.0);
    let qx = (x - half_w).abs() - half_w + radius;
    let qy = (y - half_h).abs() - half_h + radius;
    let outside = qx.max(0.0).hypot(qy.max(0.0));
    outside + qx.max(qy).min(0.0) - radius
}

fn decode(packed: u32) -> Rgba {
    Rgba::rgba(
        (packed >> 24) as u8,
        (packed >> 16) as u8,
        (packed >> 8) as u8,
        packed as u8,
    )
}

fn srgb_to_linear(value: u8) -> f32 {
    let encoded = f32::from(value) / 255.0;
    if encoded <= 0.04045 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(value: f32) -> u8 {
    let encoded = if value <= 0.0031308 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    };
    (encoded.clamp(0.0, 1.0) * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rounded_corner_has_partial_coverage_and_no_halo() {
        let image = rasterize(Key {
            width: 48,
            height: 48,
            radius: 16,
            border: 1,
            fill: 0x000000ff,
            stroke: 0x50594bff,
        });
        let bytes = image.as_bytes(0).unwrap();
        let alpha = |x: u32, y: u32| bytes[((y * 48 + x) * 4 + 3) as usize];
        assert_eq!(alpha(0, 0), 0, "the corner of the box is outside the curve");
        assert_eq!(alpha(24, 24), 255, "the middle of the card is opaque");
        assert!(
            bytes
                .chunks_exact(4)
                .any(|pixel| pixel[3] > 0 && pixel[3] < 255),
            "the curve covers some pixels only partly"
        );
        assert_eq!(
            alpha(0, 24),
            255,
            "the middle of a straight edge stays covered"
        );
    }
}
