#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Draws the RustZIP application icon and writes every file the installers need.
//!
//!     cargo run --release -p rustzip-gui --example make_icons -- packaging/icons
//!
//! The icon is a teal rounded square with a white zipper. It is drawn by distance
//! functions with 4x4 supersampling, so it is crisp at every size and the output is
//! the same on every machine.

use image::{ImageFormat, Rgba, RgbaImage};
use std::fs;
use std::io::Cursor;
use std::path::Path;

/// Signed distance to a rounded box centred at (cx, cy); negative inside.
fn rbox(x: f32, y: f32, cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let (dx, dy) = ((x - cx).abs() - (hw - r), (y - cy).abs() - (hh - r));
    dx.max(0.).hypot(dy.max(0.)) + dx.max(dy).min(0.) - r
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

/// The colour (premultiplied RGBA) of the icon at a point of the unit square.
fn sample(x: f32, y: f32) -> [f32; 4] {
    if rbox(x, y, 0.5, 0.5, 0.5, 0.5, 0.225) > 0. {
        return [0.; 4];
    }
    let teal = mix([0.10, 0.76, 0.63], [0.04, 0.50, 0.42], y);
    let mut rgb = teal;
    let white = [1.0, 1.0, 1.0];

    // The two rows of teeth, offset against each other by half a step.
    if (0.36..0.90).contains(&y) {
        let pitch = 0.085;
        for (side, shift) in [(-1.0f32, 0.0f32), (1.0, 0.5)] {
            let row = ((y - 0.36) / pitch - shift).floor();
            let cy = 0.36 + (row + shift + 0.5) * pitch;
            let cx = 0.5 + side * 0.062;
            if cy > 0.36 && cy < 0.90 && rbox(x, y, cx, cy, 0.054, 0.027, 0.016) < 0. {
                rgb = white;
            }
        }
    }
    // The slider with its pull tab.
    if rbox(x, y, 0.5, 0.235, 0.118, 0.135, 0.05) < 0. {
        rgb = white;
        if rbox(x, y, 0.5, 0.215, 0.036, 0.055, 0.03) < 0. {
            rgb = teal;
        }
    }
    [rgb[0], rgb[1], rgb[2], 1.0]
}

fn render(size: u32) -> RgbaImage {
    const S: u32 = 4;
    let mut img = RgbaImage::new(size, size);
    for py in 0..size {
        for px in 0..size {
            let mut acc = [0f32; 4];
            for sy in 0..S {
                for sx in 0..S {
                    let x = (px as f32 + (sx as f32 + 0.5) / S as f32) / size as f32;
                    let y = (py as f32 + (sy as f32 + 0.5) / S as f32) / size as f32;
                    let c = sample(x, y);
                    // Average premultiplied colour so edges blend into transparency.
                    for i in 0..3 {
                        acc[i] += c[i] * c[3];
                    }
                    acc[3] += c[3];
                }
            }
            let n = (S * S) as f32;
            let a = acc[3] / n;
            let out = if a > 0. {
                [acc[0] / acc[3], acc[1] / acc[3], acc[2] / acc[3], a]
            } else {
                [0.; 4]
            };
            img.put_pixel(
                px,
                py,
                Rgba(out.map(|v| (v.clamp(0., 1.) * 255.).round() as u8)),
            );
        }
    }
    img
}

fn png(size: u32) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    render(size).write_to(&mut out, ImageFormat::Png).unwrap();
    out.into_inner()
}

/// An Apple icon file: PNG images of the sizes macOS asks for.
fn icns() -> Vec<u8> {
    let parts: [(&[u8; 4], u32); 8] = [
        (b"ic11", 32),
        (b"ic12", 64),
        (b"ic07", 128),
        (b"ic13", 256),
        (b"ic08", 256),
        (b"ic14", 512),
        (b"ic09", 512),
        (b"ic10", 1024),
    ];
    let mut body = Vec::new();
    for (kind, size) in parts {
        let data = png(size);
        body.extend_from_slice(kind);
        body.extend_from_slice(&(data.len() as u32 + 8).to_be_bytes());
        body.extend_from_slice(&data);
    }
    let mut out = b"icns".to_vec();
    out.extend_from_slice(&(body.len() as u32 + 8).to_be_bytes());
    out.extend_from_slice(&body);
    out
}

/// A Windows icon with PNG images (supported since Vista).
fn ico() -> Vec<u8> {
    let sizes = [16u32, 24, 32, 48, 64, 128, 256];
    let images: Vec<Vec<u8>> = sizes.iter().map(|s| png(*s)).collect();
    let mut out = vec![0, 0, 1, 0];
    out.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (size, data) in sizes.iter().zip(&images) {
        out.push(if *size >= 256 { 0 } else { *size as u8 });
        out.push(if *size >= 256 { 0 } else { *size as u8 });
        out.extend_from_slice(&[0, 0, 1, 0, 32, 0]);
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        offset += data.len() as u32;
    }
    for data in images {
        out.extend_from_slice(&data);
    }
    out
}

fn main() {
    let dir = std::env::args()
        .nth(1)
        .expect("usage: make_icons <output dir>");
    let dir = Path::new(&dir);
    fs::create_dir_all(dir.join("hicolor")).unwrap();
    for size in [16, 32, 48, 64, 128, 256, 512] {
        fs::write(dir.join("hicolor").join(format!("{size}.png")), png(size)).unwrap();
    }
    fs::write(dir.join("RustZIP.icns"), icns()).unwrap();
    fs::write(dir.join("rustzip.ico"), ico()).unwrap();
    println!("wrote icons to {}", dir.display());
}
