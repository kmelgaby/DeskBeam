//! Screen frame color conversion and VP8 encode/decode.

use anyhow::{bail, Result};
use shiguredo_libvpx::{
    CodecConfig, Decoder, DecoderCodec, DecoderConfig, EncodeOptions, Encoder, EncoderConfig,
    EncodingDeadline, ImageData, ImageFormat, RateControlMode, ReconfigureParams, Vp8Config,
};
use std::num::NonZeroUsize;

/// Planar YUV 4:2:0 frame with reusable buffers.
pub struct I420Frame {
    pub width: usize,
    pub height: usize,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

impl I420Frame {
    pub fn new(width: usize, height: usize) -> Result<Self> {
        if width == 0 || height == 0 {
            bail!("empty frame");
        }
        if width % 2 != 0 || height % 2 != 0 {
            bail!("I420 requires even dimensions, got {width}x{height}");
        }
        let cw = width / 2;
        let ch = height / 2;
        Ok(Self {
            width,
            height,
            y: vec![0u8; width * height],
            u: vec![128u8; cw * ch],
            v: vec![128u8; cw * ch],
        })
    }
}

/// Convert a top-down BGRA buffer (as produced by GDI) into I420.
pub fn bgra_to_i420(bgra: &[u8], width: usize, height: usize, out: &mut I420Frame) -> Result<()> {
    if width != out.width || height != out.height {
        bail!(
            "frame size mismatch: input {width}x{height}, output {}x{}",
            out.width,
            out.height
        );
    }
    if bgra.len() < width * height * 4 {
        bail!("BGRA buffer too small: {} bytes", bgra.len());
    }
    let cw = width / 2;
    for row in 0..height {
        let src_row = row * width * 4;
        let y_row = row * width;
        for col in 0..width {
            let i = src_row + col * 4;
            let b = bgra[i] as i32;
            let g = bgra[i + 1] as i32;
            let r = bgra[i + 2] as i32;

            let y = ((66 * r + 129 * g + 25 * b + 128) >> 8) + 16;
            out.y[y_row + col] = y.clamp(0, 255) as u8;

            if row % 2 == 0 && col % 2 == 0 {
                let u = ((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128;
                let v = ((112 * r - 94 * g - 18 * b + 128) >> 8) + 128;
                let ci = (row / 2) * cw + col / 2;
                out.u[ci] = u.clamp(0, 255) as u8;
                out.v[ci] = v.clamp(0, 255) as u8;
            }
        }
    }
    Ok(())
}

/// Decoded RGBA image ready for egui.
pub struct RgbaFrame {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

fn yuv_to_rgba(
    y_plane: &[u8],
    u_plane: &[u8],
    v_plane: &[u8],
    y_stride: usize,
    u_stride: usize,
    v_stride: usize,
    width: usize,
    height: usize,
) -> Vec<u8> {
    let mut out = vec![0u8; width * height * 4];
    for row in 0..height {
        let y_row = row * y_stride;
        let c_row = (row / 2) * u_stride;
        let c_row_v = (row / 2) * v_stride;
        let out_row = row * width * 4;
        for col in 0..width {
            let c = y_plane[y_row + col] as i32 - 16;
            let d = u_plane[c_row + col / 2] as i32 - 128;
            let e = v_plane[c_row_v + col / 2] as i32 - 128;

            let r = ((298 * c + 409 * e + 128) >> 8).clamp(0, 255) as u8;
            let g = ((298 * c - 100 * d - 208 * e + 128) >> 8).clamp(0, 255) as u8;
            let b = ((298 * c + 516 * d + 128) >> 8).clamp(0, 255) as u8;

            let o = out_row + col * 4;
            out[o] = r;
            out[o + 1] = g;
            out[o + 2] = b;
            out[o + 3] = 255;
        }
    }
    out
}

pub struct Vp8Encoder {
    enc: Encoder,
    width: usize,
    height: usize,
    bitrate: usize,
}

impl Vp8Encoder {
    pub fn new(width: usize, height: usize, bitrate: u32, fps: u16) -> Result<Self> {
        if width == 0 || height == 0 {
            bail!("invalid encoder dimensions {width}x{height}");
        }
        let mut cfg = EncoderConfig::new(
            width,
            height,
            ImageFormat::I420,
            CodecConfig::Vp8(Vp8Config {
                noise_sensitivity: Some(2),
                static_threshold: Some(0),
                ..Default::default()
            }),
        );
        cfg.fps_numerator = fps.max(1) as usize;
        cfg.fps_denominator = 1;
        cfg.target_bitrate = bitrate as usize;
        cfg.min_quantizer = 0;
        cfg.max_quantizer = 50;
        cfg.cq_level = 10;
        cfg.cpu_used = Some(8);
        cfg.deadline = EncodingDeadline::Realtime;
        cfg.rate_control = RateControlMode::Cbr;
        cfg.lag_in_frames = None;
        cfg.threads = NonZeroUsize::new(2);
        cfg.error_resilient = true;
        cfg.keyframe_interval = NonZeroUsize::new((fps.max(1) as usize) * 2);
        // No internal frame dropping: the capture loop controls pacing and the
        // writer queue drops frames when the link is saturated.
        cfg.frame_drop_threshold = None;

        let enc = Encoder::new(cfg).map_err(|e| anyhow::anyhow!("VP8 encoder init failed: {e}"))?;
        Ok(Self {
            enc,
            width,
            height,
            bitrate: bitrate as usize,
        })
    }

    pub fn bitrate(&self) -> usize {
        self.bitrate
    }

    pub fn encode(&mut self, frame: &I420Frame, force_keyframe: bool) -> Result<Vec<u8>> {
        if frame.width != self.width || frame.height != self.height {
            bail!("encoder size mismatch");
        }
        let image = ImageData::I420 {
            y: &frame.y,
            u: &frame.u,
            v: &frame.v,
        };
        self.enc
            .encode(
                &image,
                &EncodeOptions {
                    force_keyframe,
                },
            )
            .map_err(|e| anyhow::anyhow!("VP8 encode failed: {e}"))?;

        let mut out = Vec::new();
        while let Some(packet) = self.enc.next_frame() {
            out.extend_from_slice(packet.data());
        }
        Ok(out)
    }

    pub fn set_bitrate(&mut self, bps: u32) -> Result<()> {
        let bps = bps.clamp(600_000, 12_000_000) as usize;
        if bps == self.bitrate {
            return Ok(());
        }
        self.enc
            .reconfigure(&ReconfigureParams {
                target_bitrate: Some(bps),
                ..Default::default()
            })
            .map_err(|e| anyhow::anyhow!("VP8 reconfigure failed: {e}"))?;
        self.bitrate = bps;
        Ok(())
    }
}

pub struct Vp8Decoder {
    dec: Decoder,
}

impl Vp8Decoder {
    pub fn new() -> Result<Self> {
        let dec = Decoder::new(DecoderConfig::new(DecoderCodec::Vp8))
            .map_err(|e| anyhow::anyhow!("VP8 decoder init failed: {e}"))?;
        Ok(Self { dec })
    }

    /// Feed one encoded frame and return the newest decoded image, if any.
    pub fn decode(&mut self, data: &[u8]) -> Result<Option<RgbaFrame>> {
        self.dec
            .decode(data)
            .map_err(|e| anyhow::anyhow!("VP8 decode failed: {e}"))?;

        let mut newest = None;
        while let Some(frame) = self
            .dec
            .next_frame()
            .map_err(|e| anyhow::anyhow!("VP8 frame fetch failed: {e}"))?
        {
            let width = frame.width();
            let height = frame.height();
            if width == 0 || height == 0 {
                continue;
            }
            let pixels = yuv_to_rgba(
                frame.y_plane(),
                frame.u_plane(),
                frame.v_plane(),
                frame.y_stride(),
                frame.u_stride(),
                frame.v_stride(),
                width,
                height,
            );
            newest = Some(RgbaFrame {
                width,
                height,
                pixels,
            });
        }
        Ok(newest)
    }
}

/// Peak signal-to-noise ratio between two RGBA images (test helper).
#[cfg(test)]
pub fn psnr_rgba(a: &RgbaFrame, b: &RgbaFrame) -> f64 {
    assert_eq!(a.pixels.len(), b.pixels.len());
    let mut mse = 0.0f64;
    let mut count = 0u64;
    for (x, y) in a.pixels.iter().zip(b.pixels.iter()) {
        let d = *x as f64 - *y as f64;
        mse += d * d;
        count += 1;
    }
    mse /= count as f64;
    if mse == 0.0 {
        return f64::INFINITY;
    }
    10.0 * (255.0f64 * 255.0 / mse).log10()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient_bgra(w: usize, h: usize) -> Vec<u8> {
        let mut buf = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 4;
                buf[i] = ((x * 255) / w) as u8; // B
                buf[i + 1] = ((y * 255) / h) as u8; // G
                buf[i + 2] = (((x + y) * 127) / (w + h)) as u8; // R
                buf[i + 3] = 255;
            }
        }
        buf
    }

    #[test]
    fn bgra_i420_sizes() {
        let (w, h) = (64, 48);
        let bgra = gradient_bgra(w, h);
        let mut i420 = I420Frame::new(w, h).unwrap();
        bgra_to_i420(&bgra, w, h, &mut i420).unwrap();
        assert_eq!(i420.y.len(), w * h);
        assert_eq!(i420.u.len(), w * h / 4);
        assert_eq!(i420.v.len(), w * h / 4);
    }

    #[test]
    fn i420_rejects_odd() {
        assert!(I420Frame::new(63, 48).is_err());
        assert!(I420Frame::new(64, 47).is_err());
    }

    #[test]
    fn vp8_encode_decode_roundtrip() {
        let (w, h) = (320, 240);
        let src_bgra = gradient_bgra(w, h);
        let mut i420 = I420Frame::new(w, h).unwrap();
        bgra_to_i420(&src_bgra, w, h, &mut i420).unwrap();

        let mut enc = Vp8Encoder::new(w, h, 2_000_000, 30).unwrap();
        let mut dec = Vp8Decoder::new().unwrap();

        let encoded = enc.encode(&i420, true).unwrap();
        assert!(!encoded.is_empty());
        let decoded = dec.decode(&encoded).unwrap().expect("decoded frame");
        assert_eq!(decoded.width, w);
        assert_eq!(decoded.height, h);

        let original = RgbaFrame {
            width: w,
            height: h,
            pixels: {
                // Convert the source through the same YUV path for a fair comparison.
                let mut tmp = vec![0u8; w * h * 4];
                for y in 0..h {
                    for x in 0..w {
                        let i = (y * w + x) * 4;
                        tmp[i] = src_bgra[i + 2];
                        tmp[i + 1] = src_bgra[i + 1];
                        tmp[i + 2] = src_bgra[i];
                        tmp[i + 3] = 255;
                    }
                }
                tmp
            },
        };
        let psnr = psnr_rgba(&original, &decoded);
        assert!(psnr > 25.0, "PSNR too low: {psnr}");
    }

    #[test]
    fn bitrate_reconfigure() {
        let mut enc = Vp8Encoder::new(64, 64, 1_000_000, 30).unwrap();
        enc.set_bitrate(2_000_000).unwrap();
        assert_eq!(enc.bitrate(), 2_000_000);
        enc.set_bitrate(100).unwrap(); // clamped
        assert_eq!(enc.bitrate(), 600_000);
    }
}
