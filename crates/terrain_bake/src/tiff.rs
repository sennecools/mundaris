//! Minimal strict reader for single-band, uncompressed float32 (Geo)TIFFs, such
//! as LROC NAC DTMs. Supports strips or tiles and either byte order; reads the
//! GeoTIFF ModelPixelScale (tag 33550) for the cell size. Anything else is
//! rejected rather than misread.

use anyhow::{Context, Result, bail, ensure};
use std::fs;
use std::path::Path;

pub struct Raster {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
    /// Cell size from ModelPixelScale (x), if present.
    pub pixel_scale: Option<f64>,
}

struct Reader<'a> {
    b: &'a [u8],
    le: bool,
}

impl Reader<'_> {
    fn u16(&self, o: usize) -> Result<u16> {
        let s: [u8; 2] = self.b.get(o..o + 2).context("tiff truncated")?.try_into()?;
        Ok(if self.le {
            u16::from_le_bytes(s)
        } else {
            u16::from_be_bytes(s)
        })
    }
    fn u32(&self, o: usize) -> Result<u32> {
        let s: [u8; 4] = self.b.get(o..o + 4).context("tiff truncated")?.try_into()?;
        Ok(if self.le {
            u32::from_le_bytes(s)
        } else {
            u32::from_be_bytes(s)
        })
    }
    fn f64(&self, o: usize) -> Result<f64> {
        let s: [u8; 8] = self.b.get(o..o + 8).context("tiff truncated")?.try_into()?;
        Ok(if self.le {
            f64::from_le_bytes(s)
        } else {
            f64::from_be_bytes(s)
        })
    }

    /// Values of one IFD entry as u64 (types SHORT=3, LONG=4).
    fn values(&self, entry: usize) -> Result<Vec<u64>> {
        let (kind, count) = (self.u16(entry + 2)?, self.u32(entry + 4)? as usize);
        let size = match kind {
            3 => 2,
            4 => 4,
            other => bail!("unsupported tiff field type {other}"),
        };
        let base = if size * count <= 4 {
            entry + 8
        } else {
            self.u32(entry + 8)? as usize
        };
        (0..count)
            .map(|k| {
                Ok(if size == 2 {
                    u64::from(self.u16(base + 2 * k)?)
                } else {
                    u64::from(self.u32(base + 4 * k)?)
                })
            })
            .collect()
    }
}

enum Storage {
    Strips {
        rows: usize,
        /// (byte offset, byte count) per strip.
        strips: Vec<(usize, usize)>,
    },
    Tiles {
        tw: usize,
        th: usize,
        across: usize,
        offsets: Vec<usize>,
    },
}

/// Parsed structure of a TIFF held in memory; extracts pixel windows without
/// materialising the whole raster.
pub struct Layout {
    pub width: usize,
    pub height: usize,
    /// Cell size from ModelPixelScale (x), if present.
    pub pixel_scale: Option<f64>,
    le: bool,
    storage: Storage,
}

impl Layout {
    pub fn parse(b: &[u8]) -> Result<Self> {
        let le = match b.get(0..2) {
            Some(b"II") => true,
            Some(b"MM") => false,
            _ => bail!("not a TIFF file"),
        };
        let r = Reader { b, le };
        ensure!(r.u16(2)? == 42, "not a classic TIFF (BigTIFF unsupported)");
        let ifd = r.u32(4)? as usize;
        let count = r.u16(ifd)? as usize;
        let mut tag = std::collections::HashMap::new();
        for i in 0..count {
            let e = ifd + 2 + i * 12;
            tag.insert(r.u16(e)?, e);
        }
        let one = |t: u16, default: Option<u64>| -> Result<u64> {
            match tag.get(&t) {
                Some(&e) => Ok(r.values(e)?[0]),
                None => default.with_context(|| format!("missing tiff tag {t}")),
            }
        };
        let (w, h) = (one(256, None)? as usize, one(257, None)? as usize);
        ensure!(one(258, None)? == 32, "only 32-bit samples supported");
        ensure!(one(259, Some(1))? == 1, "only uncompressed TIFF supported");
        ensure!(one(277, Some(1))? == 1, "only single-band TIFF supported");
        ensure!(one(339, Some(1))? == 3, "only IEEE float samples supported");
        let storage = if let (Some(&offs), Some(&counts)) = (tag.get(&273), tag.get(&279)) {
            let rows = (one(278, Some(h as u64))? as usize).max(1);
            let (offs, counts) = (r.values(offs)?, r.values(counts)?);
            ensure!(offs.len() == counts.len(), "strip tables differ");
            Storage::Strips {
                rows,
                strips: offs
                    .iter()
                    .zip(&counts)
                    .map(|(&o, &c)| (o as usize, c as usize))
                    .collect(),
            }
        } else if let (Some(&offs), Some(&counts)) = (tag.get(&324), tag.get(&325)) {
            let (tw, th) = (one(322, None)? as usize, one(323, None)? as usize);
            ensure!(tw > 0 && th > 0, "empty tiles");
            let offsets = r.values(offs)?;
            ensure!(
                offsets.len() == r.values(counts)?.len(),
                "tile tables differ"
            );
            Storage::Tiles {
                tw,
                th,
                across: w.div_ceil(tw),
                offsets: offsets.into_iter().map(|o| o as usize).collect(),
            }
        } else {
            bail!("tiff has neither strips nor tiles");
        };
        let pixel_scale = match tag.get(&33550) {
            Some(&e) => Some(r.f64(r.u32(e + 8)? as usize)?),
            None => None,
        };
        Ok(Self {
            width: w,
            height: h,
            pixel_scale,
            le,
            storage,
        })
    }

    fn sample(&self, b: &[u8], at: usize) -> Result<f32> {
        let s: [u8; 4] = b.get(at..at + 4).context("tiff truncated")?.try_into()?;
        Ok(if self.le {
            f32::from_le_bytes(s)
        } else {
            f32::from_be_bytes(s)
        })
    }

    /// Row-major window `[x, x + w) × [y, y + h)`. Pixels the file does not
    /// store (short strips) read as NaN.
    pub fn window(&self, b: &[u8], x: usize, y: usize, w: usize, h: usize) -> Result<Vec<f32>> {
        ensure!(
            x + w <= self.width && y + h <= self.height,
            "window {x},{y} {w}x{h} outside the {}x{} raster",
            self.width,
            self.height
        );
        let mut out = vec![f32::NAN; w * h];
        for row in 0..h {
            let py = y + row;
            let dst = &mut out[row * w..(row + 1) * w];
            match &self.storage {
                Storage::Strips { rows, strips } => {
                    let s = py / rows;
                    let Some(&(off, count)) = strips.get(s) else {
                        continue;
                    };
                    let first = ((py - s * rows) * self.width + x) * 4;
                    for (k, v) in dst.iter_mut().enumerate() {
                        let rel = first + 4 * k;
                        if rel + 4 <= count {
                            *v = self.sample(b, off + rel)?;
                        }
                    }
                }
                Storage::Tiles {
                    tw,
                    th,
                    across,
                    offsets,
                } => {
                    for (k, v) in dst.iter_mut().enumerate() {
                        let px = x + k;
                        let t = (py / th) * across + px / tw;
                        let Some(&off) = offsets.get(t) else {
                            continue;
                        };
                        *v = self.sample(b, off + 4 * ((py % th) * tw + px % tw))?;
                    }
                }
            }
        }
        Ok(out)
    }
}

pub fn read(path: &Path) -> Result<Raster> {
    let b = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let layout = Layout::parse(&b)?;
    let data = layout.window(&b, 0, 0, layout.width, layout.height)?;
    Ok(Raster {
        width: layout.width,
        height: layout.height,
        data,
        pixel_scale: layout.pixel_scale,
    })
}
