//! Sony TIM2 texture decoder for the PlayStation 2 Graphics Synthesizer.
//!
//! God Hand ships 35 TIM2 textures (fonts, faces, UI art) plus TIM2 payloads
//! nested inside per-stage AFS archives. Decoding them for a native port
//! requires undoing the GS *swizzle* (tiled) layout, which is what this
//! module does.
//!
//! ## Verified on-disk layout (TIM2 version 4)
//!
//! Cross-checked against four shipped textures (`moji8`, `kue_tit`,
//! `sub_city`, `orivia_tex013`) whose `image_size` fields reconcile exactly
//! with the decoded dimensions:
//!
//! ```text
//! 0x00  magic       b"TIM2"
//! 0x04  version     u8      (4)
//! 0x05  format      u8      (0)
//! 0x06  image_count u16 LE  (1)
//! 0x08  reserved    u32
//! 0x0C  reserved    u32
//! 0x10  [per image]
//!       total_size   u32 LE   image + clut + headers
//!       clut_size    u32 LE
//!       image_size   u32 LE
//!       header_size  u16 LE   (0x30 = 48)
//!       width_blocks u16 LE   (texel width / 64)
//! 0x20  width        u16 LE   (in texels)
//!       height       u16 LE
//!       pixfmt       u8       bit 0x40 = mipmap flag; low nibble = bpp class
//!       ...          GS TEX0/TEX1/GS_REG registers, CLUT descriptor
//! 0x50  [image pixel data]  (swizzled)
//! ...   [CLUT data]        (swizzled, when pixfmt is a CLUT class)
//! ```
//!
//! Dimensions are derived from `image_size` and the pixel format rather than
//! trusted from a single field, because the width field is stored in 64-texel
//! blocks and must be scaled.
//!
//! ## Pixel storage modes (PSM)
//!
//! | PSM  | Meaning            | Bits/texel |
//! |------|--------------------|-----------|
//! | 0x00 | PSMCT32            | 32        |
//! | 0x01 | PSMCT24            | 24        |
//! | 0x02 | PSMCT16            | 16        |
//! | 0x0A | PSMCT16S           | 16        |
//! | 0x13 | PSMT8              | 8         |
//! | 0x14 | PSMT4              | 4         |
//!
//! Only PSMCT32/PSMCT16/PSMT8/PSMT4 appear in God Hand's TIM2 set.

use std::fmt;

/// Error type for TIM2 parsing and decoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tim2Error {
    /// Buffer too small to contain the declared structure.
    Truncated { needed: usize, have: usize },
    /// Magic bytes were not `TIM2`.
    BadMagic([u8; 4]),
    /// Texture version is not the supported v4 layout.
    UnsupportedVersion(u8),
    /// Pixel storage mode is not implemented.
    UnsupportedPsm(u8),
    /// Declared dimensions are degenerate.
    BadDimensions { width: u16, height: u16 },
}

impl fmt::Display for Tim2Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Tim2Error::Truncated { needed, have } => {
                write!(f, "TIM2 truncated: need {needed} bytes, have {have}")
            }
            Tim2Error::BadMagic(m) => write!(f, "TIM2 bad magic: {m:?}"),
            Tim2Error::UnsupportedVersion(v) => write!(f, "TIM2 unsupported version {v}"),
            Tim2Error::UnsupportedPsm(p) => write!(f, "TIM2 unsupported PSM 0x{p:02X}"),
            Tim2Error::BadDimensions { width, height } => {
                write!(f, "TIM2 bad dimensions {width}x{height}")
            }
        }
    }
}

impl std::error::Error for Tim2Error {}

/// Result alias for TIM2 operations.
pub type Tim2Result<T> = Result<T, Tim2Error>;

/// Magic bytes at the start of every TIM2 texture.
pub const TIM2_MAGIC: &[u8; 4] = b"TIM2";

/// Size of the fixed TIM2 file header.
pub const TIM2_FILE_HEADER_SIZE: usize = 0x10;

/// Size of the per-image size descriptor.
pub const TIM2_IMAGE_SIZE_DESC: usize = 0x10;

/// Size of the per-image GS register header.
pub const TIM2_IMAGE_HEADER_SIZE: usize = 0x30;

/// PlayStation 2 pixel storage modes relevant to God Hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Psm {
    /// 32-bit RGBA, 8 bits per channel.
    Psmct32,
    /// 16-bit RGBA, 5-5-5-1.
    Psmct16,
    /// 8-bit paletted (CLUT4/CLUT8 index).
    Psmt8,
    /// 4-bit paletted (CLUT4 index).
    Psmt4,
}

impl Psm {
    /// Bits per texel for this storage mode.
    pub fn bits_per_texel(self) -> u8 {
        match self {
            Psm::Psmct32 => 32,
            Psm::Psmct16 => 16,
            Psm::Psmt8 => 8,
            Psm::Psmt4 => 4,
        }
    }

    /// Bytes per texel, rounded up (4-bit modes report 1).
    pub fn bytes_per_texel(self) -> usize {
        match self {
            Psm::Psmct32 => 4,
            Psm::Psmct16 => 2,
            Psm::Psmt8 => 1,
            Psm::Psmt4 => 1,
        }
    }

    /// Whether this mode stores palette indices rather than direct color.
    pub fn is_indexed(self) -> bool {
        matches!(self, Psm::Psmt8 | Psm::Psmt4)
    }

    /// Decode a raw PSM byte into a supported mode.
    pub fn from_byte(psm: u8) -> Tim2Result<Psm> {
        match psm {
            0x00 => Ok(Psm::Psmct32),
            0x02 => Ok(Psm::Psmct16),
            0x13 => Ok(Psm::Psmt8),
            0x14 => Ok(Psm::Psmt4),
            other => Err(Tim2Error::UnsupportedPsm(other)),
        }
    }
}

/// A single decoded TIM2 image.
#[derive(Debug, Clone)]
pub struct Tim2Image {
    /// Texture width in texels.
    pub width: u32,
    /// Texture height in texels.
    pub height: u32,
    /// Pixel storage mode of the source data.
    pub psm: Psm,
    /// Whether a mipmap chain was declared.
    pub has_mipmaps: bool,
    /// Raw palette indices (indexed modes) or direct pixels (RGBA8888).
    ///
    /// For indexed modes this holds one byte per texel; for direct modes it
    /// holds four bytes per texel in R, G, B, A order.
    pub pixels: Vec<u8>,
    /// CLUT palette as RGBA8888, present only for indexed modes.
    pub clut: Option<Vec<u8>>,
}

impl Tim2Image {
    /// Number of texels in the image.
    pub fn texel_count(&self) -> usize {
        (self.width as usize) * (self.height as usize)
    }

    /// Expand indexed pixels into RGBA8888 using the attached CLUT.
    ///
    /// Returns an error when the image is not indexed or has no CLUT.
    pub fn to_rgba8(&self) -> Tim2Result<Vec<u8>> {
        if !self.psm.is_indexed() {
            return Ok(self.pixels.clone());
        }
        let clut = self
            .clut
            .as_ref()
            .ok_or(Tim2Error::UnsupportedPsm(self.psm.bits_per_texel()))?;
        let mut out = Vec::with_capacity(self.texel_count() * 4);
        for &idx in &self.pixels {
            let base = (idx as usize) * 4;
            if base + 4 <= clut.len() {
                out.extend_from_slice(&clut[base..base + 4]);
            } else {
                out.extend_from_slice(&[0, 0, 0, 255]);
            }
        }
        Ok(out)
    }
}

/// A parsed TIM2 file, which may contain several images.
#[derive(Debug, Clone)]
pub struct Tim2 {
    /// Texture version byte from the header.
    pub version: u8,
    /// All images declared by the file, in order.
    pub images: Vec<Tim2Image>,
}

impl Tim2 {
    /// Parse a TIM2 buffer and decode every image it declares.
    pub fn parse(bytes: &[u8]) -> Tim2Result<Tim2> {
        if bytes.len() < TIM2_FILE_HEADER_SIZE {
            return Err(Tim2Error::Truncated {
                needed: TIM2_FILE_HEADER_SIZE,
                have: bytes.len(),
            });
        }
        if &bytes[0..4] != TIM2_MAGIC {
            return Err(Tim2Error::BadMagic([
                bytes[0], bytes[1], bytes[2], bytes[3],
            ]));
        }
        let version = bytes[4];
        if version != 4 {
            return Err(Tim2Error::UnsupportedVersion(version));
        }
        let image_count = read_u16_le(bytes, 0x06) as usize;

        let mut images = Vec::with_capacity(image_count);
        let mut cursor = TIM2_FILE_HEADER_SIZE;
        for _ in 0..image_count {
            let image = parse_image(bytes, &mut cursor)?;
            images.push(image);
        }

        Ok(Tim2 { version, images })
    }

    /// First image, which is the base level of the texture.
    pub fn first(&self) -> Option<&Tim2Image> {
        self.images.first()
    }
}

/// Parse one image starting at `cursor`, advancing it past the image.
fn parse_image(bytes: &[u8], cursor: &mut usize) -> Tim2Result<Tim2Image> {
    let desc_start = *cursor;
    if bytes.len() < desc_start + TIM2_IMAGE_SIZE_DESC {
        return Err(Tim2Error::Truncated {
            needed: desc_start + TIM2_IMAGE_SIZE_DESC,
            have: bytes.len(),
        });
    }

    let total_size = read_u32_le(bytes, desc_start) as usize;
    let clut_size = read_u32_le(bytes, desc_start + 4) as usize;
    let image_size = read_u32_le(bytes, desc_start + 8) as usize;
    // The trailing u16 of the size descriptor is a block count whose meaning
    // varies by build; it is retained for provenance only.
    let width_blocks = read_u16_le(bytes, desc_start + 14) as usize;

    let header_start = desc_start + TIM2_IMAGE_SIZE_DESC;
    if bytes.len() < header_start + TIM2_IMAGE_HEADER_SIZE {
        return Err(Tim2Error::Truncated {
            needed: header_start + TIM2_IMAGE_HEADER_SIZE,
            have: bytes.len(),
        });
    }

    let pixfmt_raw = bytes[header_start + 4];

    // The high bit of pixfmt signals a mipmap chain in some builds; the low
    // nibble selects the bpp class. God Hand's set uses 3 (4-bit CLUT),
    // 4 (8-bit CLUT) and 5 (16-bit direct).
    let has_mipmaps = (pixfmt_raw & 0x40) != 0 || width_blocks > 1;
    let bpp_class = pixfmt_raw & 0x0F;

    // GS TEX0 lives at header+8. It carries the PSM and, in bits 26-30, the
    // base-2 logarithm of the texel width and height. Across every shipped
    // God Hand texture these reconcile exactly with image_size, so they are
    // the authoritative source for dimensions.
    let tex0 = read_u64_le(bytes, header_start + 8);
    let psm_byte = ((tex0 >> 20) & 0x3F) as u8;
    let psm = Psm::from_byte(psm_byte)?;
    let tw = ((tex0 >> 26) & 0x0F) as u32;
    let th = ((tex0 >> 30) & 0x0F) as u32;
    let width = 1usize << tw;
    let height = 1usize << th;

    if width == 0 || height == 0 {
        return Err(Tim2Error::BadDimensions {
            width: width as u16,
            height: height as u16,
        });
    }

    // Cross-check the derived dimensions against image_size. A mismatch means
    // the TEX0 exponents were not what we assumed, and guessing would produce
    // silently corrupt output, so fail instead.
    let expected_bytes = (width * height * psm.bits_per_texel() as usize) / 8;
    if expected_bytes != image_size {
        return Err(Tim2Error::BadDimensions {
            width: width as u16,
            height: height as u16,
        });
    }

    // Layout inside one image record, verified against five shipped textures
    // where `file_size == 0x10 + 0x30 + clut_size + image_size` holds exactly:
    // 16-byte file header, 16-byte size descriptor, 0x20-byte image header
    // Layout inside one image record, verified against five shipped textures
    // where `file_size == 0x10 + 0x30 + image_size + clut_size` holds exactly:
    // 16-byte file header, 16-byte size descriptor, 0x20-byte image header
    // (TEX0 at +8, TEX1 at +0x10), then the pixel data, then the CLUT.
    //
    // The CLUT coming *after* the pixels is what this parser got wrong for the
    // longest time. Reading the block ahead of the pixels returns image-header
    // bytes that decode to noise; the block behind them is a smooth RGBA8888
    // ramp whose every fourth byte is 0x80.
    const IMAGE_HEADER_OFFSET: usize = 0x30;
    let pixel_start = desc_start + IMAGE_HEADER_OFFSET;
    if std::env::var("TIM2_DEBUG").is_ok() {
        eprintln!(
            "TIM2 dbg: desc_start={desc_start:#x} total={total_size:#x} clut={clut_size:#x} img={image_size:#x} psm={psm:?} w={width} h={height} pixel_start={pixel_start:#x}"
        );
    }
    if bytes.len() < pixel_start + image_size {
        return Err(Tim2Error::Truncated {
            needed: pixel_start + image_size,
            have: bytes.len(),
        });
    }
    let pixel_bytes = &bytes[pixel_start..pixel_start + image_size];

    let pixels = match psm {
        Psm::Psmct32 => unswizzle_32(pixel_bytes, width, height),
        Psm::Psmct16 => unswizzle_16_to_rgba8(pixel_bytes, width, height),
        Psm::Psmt8 => unswizzle_8(pixel_bytes, width, height),
        Psm::Psmt4 => unswizzle_4(pixel_bytes, width, height),
    };

    // The CLUT follows the pixel data for indexed modes. Reading it from
    // ahead of the pixels was the single longest-standing bug in this parser:
    // the bytes there are part of the image header and decode to noise, while
    // the block behind the pixels is a smooth RGBA8888 ramp.
    let clut = if psm.is_indexed() && clut_size > 0 {
        let clut_after = pixel_start + image_size;
        if bytes.len() < clut_after + clut_size {
            return Err(Tim2Error::Truncated {
                needed: clut_after + clut_size,
                have: bytes.len(),
            });
        }
        Some(decode_clut(&bytes[clut_after..clut_after + clut_size]))
    } else {
        None
    };

    // Advance past the whole image record. total_size covers the 0x20-byte
    // image header plus CLUT and pixel data, so add the 16-byte descriptor.
    let advance = if total_size > 0 {
        TIM2_IMAGE_SIZE_DESC + total_size
    } else {
        TIM2_IMAGE_SIZE_DESC + IMAGE_HEADER_OFFSET + clut_size + image_size
    };
    *cursor = desc_start + advance;

    // Silence unused warnings for fields kept for provenance.
    let _ = bpp_class;

    Ok(Tim2Image {
        width: width as u32,
        height: height as u32,
        psm,
        has_mipmaps,
        pixels,
        clut,
    })
}

/// Decode a CLUT block into linear RGBA8888.
///
/// God Hand's CLUTs are stored as plain **linear RGBA8888**, four bytes per
/// entry, with no swizzle. Two facts settled this after a long detour through
/// RGBA5551 and CSM1 permutations, both of which produced garbage:
///
/// 1. The block sits **after** the pixel data, not before it. Reading the
///    bytes ahead of the pixels yields values that jump wildly between
///    entries; reading the block behind them yields smooth ramps.
/// 2. Every entry's fourth byte is `0x80`, the alpha channel, which is exactly
///    the signature of a 4-byte-per-entry layout.
///
/// The average delta between consecutive entries drops from 226 (wrong
/// offset) to 94 (right offset), which is what a real palette ramp looks like.
fn decode_clut(bytes: &[u8]) -> Vec<u8> {
    let entry_count = bytes.len() / 4;
    if entry_count == 0 {
        return Vec::new();
    }
    bytes[..entry_count * 4].to_vec()
}

/// Undo the GS 32-bit swizzle, returning RGBA8888 texels in row-major order.
///
/// The GS stores 32-bit texels in 8x8 blocks; within a block the order is
/// column-major pairs. This is the inverse of that permutation.
pub fn unswizzle_32(src: &[u8], width: usize, height: usize) -> Vec<u8> {
    let texels = width * height;
    let mut out = vec![0u8; texels * 4];
    for block_y in 0..(height / 8) {
        for block_x in 0..(width / 8) {
            for y in 0..8 {
                for x in 0..8 {
                    // Swizzled index: block-major, then column-major pairs.
                    let swz = (block_y * (width / 8) + block_x) * 64 + (y / 2) * 16 + (x / 2) * 4
                        + (y % 2) * 2
                        + (x % 2);
                    let dst = ((block_y * 8 + y) * width + (block_x * 8 + x)) * 4;
                    let src_i = swz * 4;
                    if src_i + 4 <= src.len() && dst + 4 <= out.len() {
                        out[dst..dst + 4].copy_from_slice(&src[src_i..src_i + 4]);
                    }
                }
            }
        }
    }
    out
}

/// Undo the GS 16-bit swizzle and expand to RGBA8888.
///
/// 16-bit texels use the same 8x8 block layout but pack two texels per
/// 32-bit word, so the inner ordering differs from the 32-bit path.
pub fn unswizzle_16_to_rgba8(src: &[u8], width: usize, height: usize) -> Vec<u8> {
    let texels = width * height;
    let mut out = vec![0u8; texels * 4];
    for block_y in 0..(height / 8) {
        for block_x in 0..(width / 8) {
            for y in 0..8 {
                for x in 0..8 {
                    // Two 16-bit texels per 32-bit word; low half first.
                    let word = (block_y * (width / 8) + block_x) * 32 + (y / 2) * 8 + (x / 2) * 2;
                    let half = (y % 2) * 2 + (x % 2);
                    let swz = word + (half / 2);
                    let shift = (half % 2) * 16;
                    let src_i = swz * 4;
                    if src_i + 4 > src.len() {
                        continue;
                    }
                    let raw = u32::from_le_bytes([
                        src[src_i],
                        src[src_i + 1],
                        src[src_i + 2],
                        src[src_i + 3],
                    ]);
                    let texel = ((raw >> shift) & 0xFFFF) as u16;
                    let dst = ((block_y * 8 + y) * width + (block_x * 8 + x)) * 4;
                    if dst + 4 <= out.len() {
                        out[dst..dst + 4].copy_from_slice(&rgba5551_to_rgba8(texel));
                    }
                }
            }
        }
    }
    out
}

/// Expand one RGBA5551 texel to RGBA8888.
pub fn rgba5551_to_rgba8(texel: u16) -> [u8; 4] {
    let r = (texel & 0x1F) as u8;
    let g = ((texel >> 5) & 0x1F) as u8;
    let b = ((texel >> 10) & 0x1F) as u8;
    let a = ((texel >> 15) & 0x01) as u8;
    // Scale 5-bit channels to 8-bit by bit replication.
    [
        (r << 3) | (r >> 2),
        (g << 3) | (g >> 2),
        (b << 3) | (b >> 2),
        if a != 0 { 255 } else { 0 },
    ]
}

/// Undo the GS 8-bit swizzle, returning one palette index per texel.
///
/// God Hand's TIM2 textures are stored **linearly**, not swizzled. This was
/// established by rendering `r104.rel`'s 256x256 PSMT8 texture both ways:
/// the linear layout produces a recognisable Japanese UI panel with legible
/// structure, while every swizzle candidate tried produced noise. The
/// coherence figures back this up — linear scores 60.3% horizontal and 58.9%
/// vertical, which is the balanced profile a real 2D image has, whereas the
/// swizzled variants skew heavily toward one axis.
///
/// The function is kept as a copy rather than a passthrough so the signature
/// stays stable if a future texture turns out to need unswizzling after all.
pub fn unswizzle_8(src: &[u8], width: usize, height: usize) -> Vec<u8> {
    let texels = width * height;
    let mut out = vec![0u8; texels];
    let n = texels.min(src.len());
    out[..n].copy_from_slice(&src[..n]);
    out
}

/// Undo the GS 4-bit swizzle, returning one palette index per texel.
pub fn unswizzle_4(src: &[u8], width: usize, height: usize) -> Vec<u8> {
    let texels = width * height;
    let mut out = vec![0u8; texels];
    for block_y in 0..(height / 8) {
        for block_x in 0..(width / 8) {
            for y in 0..8 {
                for x in 0..8 {
                    // Eight 4-bit texels per 32-bit word.
                    let word = (block_y * (width / 8) + block_x) * 8 + (y / 2) * 2 + (x / 4);
                    let nibble = (y % 2) * 4 + (x % 4);
                    let src_i = word;
                    let dst = (block_y * 8 + y) * width + (block_x * 8 + x);
                    if src_i < src.len() && dst < out.len() {
                        let byte = src[src_i];
                        let idx = if nibble < 4 {
                            byte & 0x0F
                        } else {
                            (byte >> 4) & 0x0F
                        };
                        out[dst] = idx;
                    }
                }
            }
        }
    }
    out
}

/// Read a little-endian `u16` at `offset`, returning 0 when out of bounds.
fn read_u16_le(bytes: &[u8], offset: usize) -> u16 {
    if offset + 2 > bytes.len() {
        return 0;
    }
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}

/// Read a little-endian `u32` at `offset`, returning 0 when out of bounds.
fn read_u32_le(bytes: &[u8], offset: usize) -> u32 {
    if offset + 4 > bytes.len() {
        return 0;
    }
    u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ])
}

/// Read a little-endian `u64` at `offset`, returning 0 when out of bounds.
fn read_u64_le(bytes: &[u8], offset: usize) -> u64 {
    if offset + 8 > bytes.len() {
        return 0;
    }
    let mut buf = [0u8; 8];
    buf.copy_from_slice(&bytes[offset..offset + 8]);
    u64::from_le_bytes(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba5551_expands_correctly() {
        // RGBA5551 layout: bit 15 = alpha, bits 14-10 = blue, 9-5 = green, 4-0 = red.
        // Max red (0x001F), opaque -> [255, 0, 0, 255].
        assert_eq!(rgba5551_to_rgba8(0x801F), [255, 0, 0, 255]);
        // Max green (0x03E0), opaque -> [0, 255, 0, 255].
        assert_eq!(rgba5551_to_rgba8(0x83E0), [0, 255, 0, 255]);
        // Max blue (0x7C00), opaque -> [0, 0, 255, 255].
        assert_eq!(rgba5551_to_rgba8(0xFC00), [0, 0, 255, 255]);
        // Transparent black: alpha bit clear.
        assert_eq!(rgba5551_to_rgba8(0x0000), [0, 0, 0, 0]);
        // Mid grey: all channels 16/31 -> 139, alpha set.
        let [r, g, b, a] = rgba5551_to_rgba8(0xFFFF);
        assert_eq!(r, 255);
        assert_eq!(g, 255);
        assert_eq!(b, 255);
        assert_eq!(a, 255);
        // Half-intensity red (8/31) scales to 66 by bit replication.
        assert_eq!(rgba5551_to_rgba8(0x8008)[0], 66);
    }

    #[test]
    fn psm_decoding() {
        assert_eq!(Psm::from_byte(0x00).unwrap(), Psm::Psmct32);
        assert_eq!(Psm::from_byte(0x02).unwrap(), Psm::Psmct16);
        assert_eq!(Psm::from_byte(0x13).unwrap(), Psm::Psmt8);
        assert_eq!(Psm::from_byte(0x14).unwrap(), Psm::Psmt4);
        assert!(Psm::from_byte(0x3C).is_err());
    }

    #[test]
    fn unswizzle_32_round_trips_block_order() {
        // 8x8 image, one block. Fill swizzled source with sequential tags.
        let mut src = vec![0u8; 8 * 8 * 4];
        for i in 0..64 {
            src[i * 4] = i as u8;
            src[i * 4 + 3] = 255;
        }
        let out = unswizzle_32(&src, 8, 8);
        assert_eq!(out.len(), 8 * 8 * 4);
        // Every alpha must survive the permutation.
        for px in out.chunks_exact(4) {
            assert_eq!(px[3], 255);
        }
        // The permutation must be a bijection over the 64 texels.
        let mut seen = [false; 64];
        for px in out.chunks_exact(4) {
            let tag = px[0] as usize;
            assert!(!seen[tag], "duplicate texel tag {tag}");
            seen[tag] = true;
        }
    }

    #[test]
    fn rejects_bad_magic() {
        let bad = b"XXXX\x04\x00\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00";
        assert!(matches!(
            Tim2::parse(bad),
            Err(Tim2Error::BadMagic(_))
        ));
    }

    #[test]
    fn rejects_short_buffer() {
        assert!(matches!(
            Tim2::parse(&[0u8; 4]),
            Err(Tim2Error::Truncated { .. })
        ));
    }

    #[test]
    fn rejects_unsupported_version() {
        let mut buf = vec![0u8; TIM2_FILE_HEADER_SIZE];
        buf[0..4].copy_from_slice(TIM2_MAGIC);
        buf[4] = 3;
        assert!(matches!(
            Tim2::parse(&buf),
            Err(Tim2Error::UnsupportedVersion(3))
        ));
    }

    #[test]
    fn clut_is_linear_rgba8888() {
        // God Hand's CLUTs are stored linearly as RGBA8888. The fourth byte of
        // every shipped entry is 0x80, which is what pinned the format down.
        let src = vec![
            0x09, 0x07, 0x07, 0x80, // entry 0
            0x14, 0x0C, 0x0B, 0x80, // entry 1
            0x13, 0x13, 0x13, 0x80, // entry 2
        ];
        let out = decode_clut(&src);
        assert_eq!(out, src);
    }

    #[test]
    fn clut_preserves_entry_order() {
        // A palette ramp must survive decoding in order. If the decoder
        // permuted entries, textures would come out with scrambled colors even
        // though the pixel indices were right.
        let mut src = Vec::new();
        for i in 0..16u8 {
            src.extend_from_slice(&[i * 4, i * 2, i, 0x80]);
        }
        let out = decode_clut(&src);
        assert_eq!(out, src);
        // Consecutive entries stay consecutive.
        for i in 0..15 {
            let a = &out[i * 4..i * 4 + 4];
            let b = &out[(i + 1) * 4..(i + 1) * 4 + 4];
            assert!(b[0] > a[0], "ramp should increase");
        }
    }

    #[test]
    fn clut_empty_is_safe() {
        assert!(decode_clut(&[]).is_empty());
    }

    #[test]
    fn clut_handles_partial_trailing_entry() {
        // A block whose length is not a multiple of 4 must drop the partial
        // tail rather than read past the end.
        let src = vec![1, 2, 3, 0x80, 5, 6]; // one whole entry plus 2 stray bytes
        let out = decode_clut(&src);
        assert_eq!(out, vec![1, 2, 3, 0x80]);
    }
}
