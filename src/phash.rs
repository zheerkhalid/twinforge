//! Perceptual picture hashes, written for TwinForge.
//!
//! Every picture is reduced to a 32 x 32 grey matrix. Two 64-bit hashes can be
//! taken from it:
//!
//! * the **perceptual** hash: a 2-D cosine transform, of which the 8 x 8 block
//!   of lowest frequencies is compared with its median. It describes the
//!   coarse structure of the picture and barely changes when the picture is
//!   resized, recompressed or made a little brighter;
//! * the **gradient** hash: the matrix is shrunk to 9 x 8 and each cell is
//!   compared with its right neighbour.
//!
//! Hashes are compared by counting differing bits. Because rotating the small
//! matrix is cheap, a picture carries the hashes of all eight orientations
//! (four rotations, each mirrored or not), so rotated copies can be found.

use crate::model::ImageHash;

/// Side length of the grey matrix.
pub const SIDE: usize = 32;

/// A picture shrunk to 32 x 32 brightness values (0.0..=255.0), row by row.
#[derive(Clone, Debug, PartialEq)]
pub struct Gray {
    pub px: Vec<f32>,
}

impl Gray {
    /// Shrinks any picture to the grey matrix. Aspect ratio is deliberately
    /// ignored: every picture is squeezed into the square.
    pub fn from_image(img: &image::DynamicImage) -> Gray {
        // `thumbnail_exact` averages source pixels, which is what a hash wants.
        let small = img.thumbnail_exact(SIDE as u32, SIDE as u32).to_rgb8();
        let px = small
            .pixels()
            .map(|p| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32)
            .collect();
        Gray { px }
    }

    /// Builds the matrix from 1024 brightness values (tests).
    pub fn from_values(px: Vec<f32>) -> Gray {
        assert_eq!(px.len(), SIDE * SIDE);
        Gray { px }
    }

    fn at(&self, x: usize, y: usize) -> f32 {
        self.px[y * SIDE + x]
    }

    pub fn mean(&self) -> f32 {
        self.px.iter().sum::<f32>() / self.px.len() as f32
    }

    /// Standard deviation of the brightness: near zero for a blank picture.
    pub fn spread(&self) -> f32 {
        let m = self.mean();
        (self.px.iter().map(|v| (v - m) * (v - m)).sum::<f32>() / self.px.len() as f32).sqrt()
    }

    /// One of the eight orientations: `turns` quarter turns clockwise, then
    /// optionally mirrored left to right.
    pub fn oriented(&self, turns: u8, mirror: bool) -> Gray {
        let n = SIDE;
        let mut px = vec![0.0f32; n * n];
        for y in 0..n {
            for x in 0..n {
                // Where does destination (x, y) come from?
                let x = if mirror { n - 1 - x } else { x };
                let (sx, sy) = match turns % 4 {
                    0 => (x, y),
                    1 => (y, n - 1 - x),
                    2 => (n - 1 - x, n - 1 - y),
                    _ => (n - 1 - y, x),
                };
                let dx = if mirror { n - 1 - x } else { x };
                px[y * n + dx] = self.at(sx, sy);
            }
        }
        Gray { px }
    }
}

/// The cosine table for a 32-point DCT-II, rows 0..8 only (the low
/// frequencies are all the hash uses).
fn cos_table() -> [[f32; SIDE]; 8] {
    let mut t = [[0.0f32; SIDE]; 8];
    for (k, row) in t.iter_mut().enumerate() {
        for (n, v) in row.iter_mut().enumerate() {
            *v = (std::f32::consts::PI * (2 * n + 1) as f32 * k as f32 / (2 * SIDE) as f32).cos();
        }
    }
    t
}

/// The 8 x 8 block of lowest-frequency DCT coefficients of the matrix.
fn low_frequencies(g: &Gray) -> [f32; 64] {
    let cos = cos_table();
    // Rows first: tmp[y][k] = sum over x.
    let mut tmp = [[0.0f32; 8]; SIDE];
    for (y, row) in tmp.iter_mut().enumerate() {
        for (k, out) in row.iter_mut().enumerate() {
            let mut s = 0.0;
            for (x, c) in cos[k].iter().enumerate() {
                s += g.at(x, y) * c;
            }
            *out = s;
        }
    }
    let mut out = [0.0f32; 64];
    for ky in 0..8 {
        for kx in 0..8 {
            let mut s = 0.0;
            for (y, row) in tmp.iter().enumerate() {
                s += row[kx] * cos[ky][y];
            }
            out[ky * 8 + kx] = s;
        }
    }
    out
}

/// The DCT-based hash of the matrix.
pub fn perceptual(g: &Gray) -> u64 {
    let c = low_frequencies(g);
    // The very first coefficient is the overall brightness; leave it out of
    // the median so that a brighter copy keeps its hash.
    let mut sorted: Vec<f32> = c[1..].to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let median = (sorted[31] + sorted[32]) / 2.0;
    let mut bits = 0u64;
    for (i, v) in c.iter().enumerate().skip(1) {
        if *v > median {
            bits |= 1 << i;
        }
    }
    bits
}

/// Shrinks the matrix to `w` x `h` by averaging the covered cells.
fn shrink(g: &Gray, w: usize, h: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; w * h];
    for oy in 0..h {
        let (y0, y1) = (
            oy as f32 * SIDE as f32 / h as f32,
            (oy + 1) as f32 * SIDE as f32 / h as f32,
        );
        for ox in 0..w {
            let (x0, x1) = (
                ox as f32 * SIDE as f32 / w as f32,
                (ox + 1) as f32 * SIDE as f32 / w as f32,
            );
            let (mut sum, mut weight) = (0.0f32, 0.0f32);
            for y in y0.floor() as usize..(y1.ceil() as usize).min(SIDE) {
                let wy = (y1.min(y as f32 + 1.0) - y0.max(y as f32)).max(0.0);
                for x in x0.floor() as usize..(x1.ceil() as usize).min(SIDE) {
                    let wx = (x1.min(x as f32 + 1.0) - x0.max(x as f32)).max(0.0);
                    sum += g.at(x, y) * wx * wy;
                    weight += wx * wy;
                }
            }
            out[oy * w + ox] = if weight > 0.0 { sum / weight } else { 0.0 };
        }
    }
    out
}

/// The gradient hash of the matrix.
pub fn gradient(g: &Gray) -> u64 {
    let small = shrink(g, 9, 8);
    let mut bits = 0u64;
    for y in 0..8 {
        for x in 0..8 {
            // The small margin keeps even areas from producing noise bits.
            if small[y * 9 + x] > small[y * 9 + x + 1] + 0.25 {
                bits |= 1 << (y * 8 + x);
            }
        }
    }
    bits
}

/// Everything the comparison needs to know about one picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Signature {
    /// Hashes of the eight orientations; `hashes[0]` is the picture as it is.
    pub hashes: [u64; 8],
    /// The picture is (nearly) one flat colour: its hash bits are noise.
    pub flat: bool,
    /// Average brightness 0..=255 (used to compare flat pictures).
    pub mean: u8,
}

/// Below this brightness spread a picture counts as blank.
const FLAT_SPREAD: f32 = 2.5;

impl Signature {
    pub fn of(gray: &Gray, kind: ImageHash) -> Signature {
        let hash = |g: &Gray| match kind {
            ImageHash::Perceptual => perceptual(g),
            ImageHash::Gradient => gradient(g),
        };
        let mut hashes = [0u64; 8];
        for (i, h) in hashes.iter_mut().enumerate() {
            *h = if i == 0 {
                hash(gray)
            } else {
                hash(&gray.oriented((i % 4) as u8, i >= 4))
            };
        }
        Signature {
            hashes,
            flat: gray.spread() < FLAT_SPREAD,
            mean: gray.mean().round().clamp(0.0, 255.0) as u8,
        }
    }

    pub fn of_image(img: &image::DynamicImage, kind: ImageHash) -> Signature {
        Signature::of(&Gray::from_image(img), kind)
    }

    /// Differing bits (0..=64) between this picture and `other`. With
    /// `rotated`, the best of the eight orientations counts.
    pub fn distance(&self, other: &Signature, rotated: bool) -> u32 {
        if self.flat || other.flat {
            // Blank pictures only resemble blank pictures of the same shade.
            return if self.flat && other.flat && self.mean.abs_diff(other.mean) <= 6 {
                0
            } else {
                64
            };
        }
        let base = other.hashes[0];
        if rotated {
            self.hashes
                .iter()
                .map(|h| (h ^ base).count_ones())
                .min()
                .unwrap_or(64)
        } else {
            (self.hashes[0] ^ base).count_ones()
        }
    }
}

/// Similarity in percent for a number of differing bits.
pub fn similarity(distance: u32) -> u8 {
    (100.0 - distance.min(64) as f32 * 100.0 / 64.0).round() as u8
}

/// The largest number of differing bits that still counts as "similar" for a
/// similarity setting of 60..=100 percent.
pub fn max_distance(similarity_percent: u8) -> u32 {
    let s = similarity_percent.clamp(50, 100) as f32;
    ((100.0 - s) * 64.0 / 100.0).floor() as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selftest::picture;
    use image::{imageops, DynamicImage};

    fn sig(img: &image::RgbImage) -> Signature {
        Signature::of_image(&DynamicImage::ImageRgb8(img.clone()), ImageHash::Perceptual)
    }

    fn jpeg_round_trip(img: &image::RgbImage, quality: u8) -> image::RgbImage {
        let mut bytes = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, quality)
            .encode_image(img)
            .unwrap();
        image::load_from_memory(&bytes).unwrap().to_rgb8()
    }

    #[test]
    fn thresholds_are_monotonic() {
        assert_eq!(max_distance(100), 0);
        assert_eq!(max_distance(90), 6);
        assert_eq!(max_distance(80), 12);
        let mut last = 0;
        for s in (60..=100).rev() {
            assert!(max_distance(s) >= last);
            last = max_distance(s);
        }
        assert_eq!(similarity(0), 100);
        assert_eq!(similarity(64), 0);
        assert_eq!(similarity(6), 91);
        // Whatever passes a setting also shows at least that percentage.
        for s in 60..=100u8 {
            assert!(similarity(max_distance(s)) >= s);
        }
    }

    #[test]
    fn resized_recompressed_and_brightened_copies_match() {
        for seed in [3, 5, 11, 20] {
            let base = picture(seed, 640, 480);
            let a = sig(&base);
            let limit = max_distance(90);
            let half = imageops::resize(&base, 320, 240, imageops::FilterType::Triangle);
            let double = imageops::resize(&base, 1280, 960, imageops::FilterType::CatmullRom);
            let jpeg = jpeg_round_trip(&base, 60);
            let bright = image::RgbImage::from_fn(640, 480, |x, y| {
                let p = base.get_pixel(x, y);
                image::Rgb([
                    p[0].saturating_add(14),
                    p[1].saturating_add(14),
                    p[2].saturating_add(14),
                ])
            });
            for (name, copy) in [
                ("half", &half),
                ("double", &double),
                ("jpeg", &jpeg),
                ("bright", &bright),
            ] {
                let d = a.distance(&sig(copy), false);
                assert!(d <= limit, "seed {seed} {name}: {d} bits differ");
            }
            // The gradient hash copes with the same changes.
            let g = |i: &image::RgbImage| {
                Signature::of_image(&DynamicImage::ImageRgb8(i.clone()), ImageHash::Gradient)
            };
            assert!(g(&base).distance(&g(&half), false) <= limit);
            assert!(g(&base).distance(&g(&jpeg), false) <= limit);
        }
    }

    #[test]
    fn different_pictures_do_not_match() {
        let seeds = [3u32, 5, 11, 20, 31, 42];
        let limit = max_distance(80);
        for (i, a) in seeds.iter().enumerate() {
            for b in &seeds[i + 1..] {
                let (sa, sb) = (sig(&picture(*a, 640, 480)), sig(&picture(*b, 640, 480)));
                let d = sa.distance(&sb, true);
                assert!(d > limit, "seeds {a} and {b} differ by only {d} bits");
            }
        }
    }

    #[test]
    fn rotated_and_mirrored_copies_match_only_on_request() {
        let base = picture(5, 640, 480);
        let a = sig(&base);
        let limit = max_distance(90);
        let copies = [
            ("90", imageops::rotate90(&base)),
            ("180", imageops::rotate180(&base)),
            ("270", imageops::rotate270(&base)),
            ("mirror", imageops::flip_horizontal(&base)),
            ("flip", imageops::flip_vertical(&base)),
        ];
        for (name, copy) in &copies {
            let s = sig(copy);
            assert!(
                a.distance(&s, true) <= limit && s.distance(&a, true) <= limit,
                "{name}: {} bits",
                a.distance(&s, true)
            );
            assert!(
                a.distance(&s, false) > limit,
                "{name} must not match without the option: {} bits",
                a.distance(&s, false)
            );
        }
    }

    #[test]
    fn orientations_of_the_matrix() {
        let g = Gray::from_values((0..SIDE * SIDE).map(|i| (i % 251) as f32).collect());
        assert_eq!(g.oriented(0, false), g);
        assert_eq!(g.oriented(1, false).oriented(3, false), g);
        assert_eq!(g.oriented(2, false).oriented(2, false), g);
        assert_eq!(g.oriented(0, true).oriented(0, true), g);
        // A quarter turn clockwise moves the top-left corner to the top-right.
        assert_eq!(g.oriented(1, false).at(SIDE - 1, 0), g.at(0, 0));
        // Mirroring swaps left and right.
        assert_eq!(g.oriented(0, true).at(0, 5), g.at(SIDE - 1, 5));
    }

    #[test]
    fn blank_pictures_only_match_blank_pictures_of_the_same_shade() {
        let flat = |v: u8| {
            sig(&image::RgbImage::from_pixel(
                200,
                100,
                image::Rgb([v, v, v]),
            ))
        };
        let (white, also_white, black) = (flat(250), flat(247), flat(4));
        assert!(white.flat && black.flat);
        assert_eq!(white.distance(&also_white, true), 0);
        assert_eq!(white.distance(&black, true), 64);
        let real = sig(&picture(3, 320, 240));
        assert!(!real.flat);
        assert_eq!(white.distance(&real, true), 64);
        assert_eq!(real.distance(&black, false), 64);
    }

    #[test]
    fn known_patterns() {
        // Left half dark, right half bright: every gradient bit in the middle
        // comparison says "left is darker or equal", none "left is brighter".
        let g = Gray::from_values(
            (0..SIDE * SIDE)
                .map(|i| if i % SIDE < SIDE / 2 { 10.0 } else { 240.0 })
                .collect(),
        );
        assert_eq!(gradient(&g), 0);
        // Mirrored, the edge falls inside one of the nine columns, so two
        // neighbouring comparisons per row see "left is brighter".
        assert_eq!(gradient(&g.oriented(0, true)).count_ones(), 16);
        assert!((g.mean() - 125.0).abs() < 0.01);
        assert!(g.spread() > 100.0);
        // The transform of a constant matrix has no structure at all.
        let c = low_frequencies(&Gray::from_values(vec![80.0; SIDE * SIDE]));
        assert!(c[1..].iter().all(|v| v.abs() < 0.05));
    }
}
