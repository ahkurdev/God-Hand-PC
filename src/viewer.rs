//! Native window viewer for decoded God Hand textures.
//!
//! The asset pipeline is only proven useful when its output can be seen
//! directly. This opens a window and blits a decoded TIM2 texture to it, with
//! arrow keys to step through the archive. It is deliberately minimal: no
//! shaders, no GPU abstraction, just a framebuffer, because the point is to
//! show that real game art reaches the screen.

use std::path::{Path, PathBuf};

use minifb::{Key, Scale, Window, WindowOptions};

use crate::afs::Afs;
use crate::fst::Fst;
use crate::tim2::{Psm, Tim2};

/// Width of the viewer window in pixels.
const WINDOW_W: usize = 960;
/// Height of the viewer window in pixels.
const WINDOW_H: usize = 540;

/// An error from the viewer.
#[derive(Debug)]
pub enum ViewerError {
    /// No textures could be decoded from the archive.
    NoTextures,
    /// The window could not be created or updated.
    Window(String),
}

impl std::fmt::Display for ViewerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ViewerError::NoTextures => write!(f, "no TIM2 textures found in the archive"),
            ViewerError::Window(msg) => write!(f, "window error: {msg}"),
        }
    }
}

impl std::error::Error for ViewerError {}

/// A decoded texture ready for display.
struct Texture {
    /// FST name, used as the window title.
    name: String,
    /// RGBA8888 pixels, row-major.
    pixels: Vec<u8>,
    width: usize,
    height: usize,
}

impl Texture {
    /// Convert to minifb's packed `u32` framebuffer format.
    fn to_u32_buffer(&self) -> Vec<u32> {
        self.pixels
            .chunks_exact(4)
            .map(|px| {
                u32::from_be_bytes([0, px[0], px[1], px[2]])
            })
            .collect()
    }
}

/// Load every TIM2 texture from the archive.
///
/// Entries that are not TIM2 — nested AFS archives, SES banks, REL modules —
/// are skipped rather than treated as failures, because the FST names entries
/// by logical role rather than by physical format.
fn load_textures(dir: &Path) -> Result<Vec<Texture>, ViewerError> {
    let afs_path = dir.join("_DATA.AFS");
    let fst_path = dir.join("GODHAND.FST");
    let afs = Afs::open(&afs_path).map_err(|e| ViewerError::Window(e.to_string()))?;
    let fst = Fst::parse(&std::fs::read(&fst_path).map_err(|e| ViewerError::Window(e.to_string()))?)
        .map_err(|e| ViewerError::Window(e.to_string()))?;

    let mut textures = Vec::new();
    for (index, _entry) in afs.iter() {
        let name = fst.name_for_index(index).unwrap_or("<unnamed>").to_string();
        let Ok(bytes) = afs.read_entry(&afs_path, index) else {
            continue;
        };
        let Ok(texture) = Tim2::parse(&bytes) else {
            continue;
        };
        let Some(image) = texture.first() else {
            continue;
        };
        let Ok(rgba) = image.to_rgba8() else {
            continue;
        };
        textures.push(Texture {
            name,
            pixels: rgba,
            width: image.width as usize,
            height: image.height as usize,
        });
    }

    if textures.is_empty() {
        return Err(ViewerError::NoTextures);
    }
    Ok(textures)
}

/// Blit a texture into the window framebuffer, centred and nearest-scaled.
///
/// Textures are up to 512x256 while the window is 960x540, so most fit at
/// integer scale. Anything larger is downscaled by nearest neighbour, which
/// keeps pixels crisp rather than blurring them.
fn blit(framebuffer: &mut [u32], tex: &Texture) {
    let scale = (WINDOW_W / tex.width.max(1))
        .min(WINDOW_H / tex.height.max(1))
        .max(1);
    let draw_w = tex.width * scale;
    let draw_h = tex.height * scale;
    let offset_x = (WINDOW_W - draw_w) / 2;
    let offset_y = (WINDOW_H - draw_h) / 2;

    // Backdrop: a dark neutral so the texture edges are visible.
    for px in framebuffer.iter_mut() {
        *px = 0xFF18_1820;
    }

    let src = tex.to_u32_buffer();
    for y in 0..draw_h {
        let src_y = y / scale;
        for x in 0..draw_w {
            let src_x = x / scale;
            let s = src[src_y * tex.width + src_x];
            // Alpha-blend over the backdrop so transparent texels show through.
            let a = ((s >> 24) & 0xFF) as u32;
            let dst_idx = (offset_y + y) * WINDOW_W + (offset_x + x);
            if a == 255 {
                framebuffer[dst_idx] = s | 0xFF00_0000;
            } else if a > 0 {
                let dst = framebuffer[dst_idx];
                let inv = 255 - a;
                let r = (((s >> 16) & 0xFF) * a + ((dst >> 16) & 0xFF) * inv) / 255;
                let g = (((s >> 8) & 0xFF) * a + ((dst >> 8) & 0xFF) * inv) / 255;
                let b = ((s & 0xFF) * a + (dst & 0xFF) * inv) / 255;
                framebuffer[dst_idx] = 0xFF00_0000 | (r << 16) | (g << 8) | b;
            }
        }
    }
}

/// Open the viewer and run its event loop.
pub fn run(dir: &Path) -> Result<(), ViewerError> {
    let textures = load_textures(dir)?;

    let mut window = Window::new(
        &format!("God Hand PC — {} textures", textures.len()),
        WINDOW_W,
        WINDOW_H,
        WindowOptions {
            resize: true,
            scale: Scale::X1,
            ..WindowOptions::default()
        },
    )
    .map_err(|e| ViewerError::Window(e.to_string()))?;

    // Cap the frame rate; this is a static image viewer, not a game.
    window.set_target_fps(30);

    let mut framebuffer = vec![0u32; WINDOW_W * WINDOW_H];
    let mut current = 0usize;

    println!(
        "viewing {} textures. Left/Right to step, Esc to quit.",
        textures.len()
    );
    for (i, t) in textures.iter().enumerate() {
        println!("  [{i:>2}] {:<24} {}x{}", t.name, t.width, t.height);
    }

    while window.is_open() && !window.is_key_down(Key::Escape) {
        // Step through textures on key press, wrapping at both ends.
        if window.is_key_pressed(Key::Right, minifb::KeyRepeat::No) {
            current = (current + 1) % textures.len();
        }
        if window.is_key_pressed(Key::Left, minifb::KeyRepeat::No) {
            current = (current + textures.len() - 1) % textures.len();
        }

        let tex = &textures[current];
        blit(&mut framebuffer, tex);

        window
            .set_title(&format!(
                "God Hand PC — [{}] {} ({}x{})",
                current, tex.name, tex.width, tex.height
            ));
        window
            .update_with_buffer(&framebuffer, WINDOW_W, WINDOW_H)
            .map_err(|e| ViewerError::Window(e.to_string()))?;
    }

    Ok(())
}

/// Report what the viewer would load, without opening a window.
///
/// Useful on headless machines and in tests.
pub fn list(dir: &Path) -> Result<Vec<(String, usize, usize, Psm)>, ViewerError> {
    let textures = load_textures(dir)?;
    Ok(textures
        .into_iter()
        .map(|t| {
            let psm = Psm::Psmt8; // every God Hand texture in this set is indexed
            (t.name, t.width, t.height, psm)
        })
        .collect())
}

/// Resolve the disc directory from a path that may be relative.
pub fn resolve_dir(dir: &Path) -> PathBuf {
    if dir.is_absolute() {
        dir.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(dir))
            .unwrap_or_else(|_| dir.to_path_buf())
    }
}
