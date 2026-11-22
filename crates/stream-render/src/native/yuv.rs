//! BGRA to planar YUV 4:2:0, BT.709, limited range: what the encoder is fed. The compositor converts
//! a frame only when it drew one: a frame that repeats the last (a still board, a card) is the same
//! buffer again, and the encoder is given 3/8 of the bytes a BGRA frame would be.
//!
//! Chroma is the average of each 2 x 2 block of pixels. Fixed-point with 16 fractional bits, in
//! safe Rust the compiler can vectorise.

/// Bytes of a `width` x `height` frame in this layout (`width` and `height` even).
pub const fn frame_len(width: usize, height: usize) -> usize {
    width * height * 3 / 2
}

/// Converts `bgra` (`width * 4` bytes a row) into `out`: the Y plane, then U, then V.
pub fn bgra_to_i420(bgra: &[u8], width: usize, height: usize, out: &mut [u8]) {
    debug_assert!(width.is_multiple_of(2) && height.is_multiple_of(2));
    debug_assert_eq!(bgra.len(), width * height * 4);
    debug_assert_eq!(out.len(), frame_len(width, height));
    let (y_plane, chroma) = out.split_at_mut(width * height);
    let (u_plane, v_plane) = chroma.split_at_mut(width * height / 4);
    for (row, y_row) in y_plane.chunks_exact_mut(width).enumerate() {
        let src = &bgra[row * width * 4..(row + 1) * width * 4];
        let (pixels, _) = src.as_chunks::<4>();
        for (y, px) in y_row.iter_mut().zip(pixels) {
            let (b, g, r) = (i32::from(px[0]), i32::from(px[1]), i32::from(px[2]));
            *y = ((11966 * r + 40254 * g + 4063 * b + (16 << 16) + 32768) >> 16) as u8;
        }
    }
    let half = width / 2;
    for row in 0..height / 2 {
        let top = &bgra[2 * row * width * 4..(2 * row + 1) * width * 4];
        let bottom = &bgra[(2 * row + 1) * width * 4..(2 * row + 2) * width * 4];
        let u_row = &mut u_plane[row * half..(row + 1) * half];
        let v_row = &mut v_plane[row * half..(row + 1) * half];
        for (col, (u, v)) in u_row.iter_mut().zip(v_row.iter_mut()).enumerate() {
            let at = col * 8;
            let sum = |k: usize| {
                i32::from(top[at + k])
                    + i32::from(top[at + 4 + k])
                    + i32::from(bottom[at + k])
                    + i32::from(bottom[at + 4 + k])
            };
            let (b, g, r) = (sum(0), sum(1), sum(2));
            // The sums are four pixels: the coefficients are for one, so 18 bits are dropped.
            let round = (128 << 18) + (1 << 17);
            *u = ((-6596 * r - 22189 * g + 28784 * b + round) >> 18) as u8;
            *v = ((28784 * r - 26145 * g - 2639 * b + round) >> 18) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat(b: u8, g: u8, r: u8) -> [u8; 3] {
        let bgra: Vec<u8> = (0..4 * 4).flat_map(|_| [b, g, r, 255]).collect();
        let mut out = vec![0; frame_len(4, 4)];
        bgra_to_i420(&bgra, 4, 4, &mut out);
        [out[0], out[16], out[20]]
    }

    #[test]
    fn the_primaries_land_where_bt709_limited_range_puts_them() {
        assert_eq!(flat(0, 0, 0), [16, 128, 128]);
        assert_eq!(flat(255, 255, 255), [235, 128, 128]);
        assert_eq!(flat(0, 0, 255), [63, 102, 240]);
        assert_eq!(flat(0, 255, 0), [173, 42, 26]);
        assert_eq!(flat(255, 0, 0), [32, 240, 118]);
    }

    #[test]
    fn a_frame_is_three_eighths_of_a_bgra_frame() {
        assert_eq!(frame_len(1280, 720) * 8, 1280 * 720 * 4 * 3);
    }

    #[test]
    fn chroma_is_the_average_of_its_block() {
        // A 2 x 2 frame of two black and two white pixels: chroma is the middle, luma is per pixel.
        let bgra = [
            0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 255, 255, 255, 255, 255,
        ];
        let mut out = vec![0; frame_len(2, 2)];
        bgra_to_i420(&bgra, 2, 2, &mut out);
        assert_eq!(out, vec![16, 235, 16, 235, 128, 128]);
    }

    /// The same frame through ffmpeg's own conversion (`RENDER_TEST_FFMPEG`): the planes agree to
    /// within a level or two (ffmpeg's chroma filter is not a box average).
    #[test]
    #[ignore = "needs ffmpeg (RENDER_TEST_FFMPEG)"]
    fn it_agrees_with_ffmpegs_conversion() {
        use std::process::Command;
        let ffmpeg = std::env::var("RENDER_TEST_FFMPEG").expect("RENDER_TEST_FFMPEG");
        let run = |pix: &str, vf: &str| {
            let out = Command::new(&ffmpeg)
                .args([
                    "-v",
                    "error",
                    "-f",
                    "lavfi",
                    "-i",
                    "testsrc2=s=1280x720:r=1:d=1",
                ])
                .args([
                    "-vf",
                    vf,
                    "-frames:v",
                    "1",
                    "-f",
                    "rawvideo",
                    "-pix_fmt",
                    pix,
                    "-",
                ])
                .output()
                .unwrap();
            assert!(out.status.success());
            out.stdout
        };
        let bgra = run("bgra", "format=bgra");
        let reference = run(
            "yuv420p",
            "scale=out_color_matrix=bt709:out_range=tv,format=yuv420p",
        );
        let mut mine = vec![0; frame_len(1280, 720)];
        bgra_to_i420(&bgra, 1280, 720, &mut mine);
        assert_eq!(mine.len(), reference.len());
        let (mut worst, mut sum) = (0i32, 0i64);
        for (a, b) in mine.iter().zip(&reference) {
            let d = (i32::from(*a) - i32::from(*b)).abs();
            worst = worst.max(d);
            sum += i64::from(d);
        }
        let mean = sum as f64 / mine.len() as f64;
        assert!(mean < 0.6, "mean difference {mean}");
        assert!(worst < 40, "worst difference {worst} (edges of chroma)");
    }
}
