//! Export, image extraction, PDF creation from images, and document optimization
//! (GOAL.md §3.8, FINISH.md S10).

use std::collections::BTreeMap;
use std::io::Write as _;

use mangle_content::Resources;
use mangle_crypto::sha256;
use mangle_doc::Page;
use mangle_filters::{DeflateLevel, deflate};
use mangle_render::{Image, RenderOptions, render_page};
use mangle_syntax::document::Document;
use mangle_syntax::object::{Dict, Object, Ref, Stream};
use mangle_syntax::save::{SaveMode, SaveOptions};
use mangle_syntax::stream::decode_stream;

use crate::writeback::SaveError;

/// Encode an RGBA [`Image`] as an 8-bit RGBA PNG using in-house zlib compression (`ADR-0006`).
#[must_use]
pub fn encode_png(image: &Image) -> Vec<u8> {
    let w = u32::try_from(image.width).unwrap_or(1);
    let h = u32::try_from(image.height).unwrap_or(1);
    let row_bytes = image.width * 4;
    let mut raw_scanlines = Vec::with_capacity((row_bytes + 1) * image.height);
    for row in 0..image.height {
        raw_scanlines.push(0u8); // Filter type 0 (None)
        if let Some(slice) = image.pixels.get(row * row_bytes..(row + 1) * row_bytes) {
            raw_scanlines.extend_from_slice(slice);
        } else {
            raw_scanlines.resize(raw_scanlines.len() + row_bytes, 0);
        }
    }
    let compressed = deflate(&raw_scanlines, DeflateLevel::Default);

    let mut out = Vec::with_capacity(64 + compressed.len());
    out.extend_from_slice(b"\x89PNG\r\n\x1a\n");

    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.push(8); // bit depth: 8
    ihdr.push(6); // color type: 6 (RGBA)
    ihdr.push(0); // compression: 0
    ihdr.push(0); // filter: 0
    ihdr.push(0); // interlace: 0
    write_png_chunk(&mut out, *b"IHDR", &ihdr);
    write_png_chunk(&mut out, *b"IDAT", &compressed);
    write_png_chunk(&mut out, *b"IEND", &[]);
    out
}

fn write_png_chunk(out: &mut Vec<u8>, tag: [u8; 4], data: &[u8]) {
    let len = u32::try_from(data.len()).unwrap_or(0);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(&tag);
    out.extend_from_slice(data);
    let crc = crc32(&tag, data);
    out.extend_from_slice(&crc.to_be_bytes());
}

fn crc32(tag: &[u8], data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in tag.iter().chain(data.iter()) {
        crc ^= u32::from(b);
        for _ in 0..8 {
            if (crc & 1) != 0 {
                crc = (crc >> 1) ^ 0xEDB8_8320;
            } else {
                crc >>= 1;
            }
        }
    }
    !crc
}

/// Export `page` to PNG at the requested `dpi` (`S10.1`).
#[must_use]
pub fn export_page_png(doc: &Document, page: &Page, resources: &Resources, dpi: f64) -> Vec<u8> {
    let scale = (dpi / 72.0).clamp(0.1, 16.0);
    let rendered = render_page(
        doc,
        page,
        resources,
        RenderOptions {
            scale,
            ..RenderOptions::default()
        },
    );
    encode_png(&rendered.image)
}

/// Export `page` to JPEG at the requested `dpi` and `quality` (`1..=100`) (`S10.1`).
pub fn export_page_jpeg(
    doc: &Document,
    page: &Page,
    resources: &Resources,
    dpi: f64,
    quality: u8,
) -> Result<Vec<u8>, String> {
    let scale = (dpi / 72.0).clamp(0.1, 16.0);
    let rendered = render_page(
        doc,
        page,
        resources,
        RenderOptions {
            scale,
            ..RenderOptions::default()
        },
    );
    let w =
        u16::try_from(rendered.image.width).map_err(|_| "image too wide for JPEG".to_string())?;
    let h =
        u16::try_from(rendered.image.height).map_err(|_| "image too tall for JPEG".to_string())?;
    let mut rgb = Vec::with_capacity(rendered.image.width * rendered.image.height * 3);
    for px in rendered.image.pixels.chunks_exact(4) {
        rgb.push(px.first().copied().unwrap_or(255));
        rgb.push(px.get(1).copied().unwrap_or(255));
        rgb.push(px.get(2).copied().unwrap_or(255));
    }
    let mut buf = Vec::new();
    let encoder = jpeg_encoder::Encoder::new(&mut buf, quality.clamp(1, 100));
    encoder
        .encode(&rgb, w, h, jpeg_encoder::ColorType::Rgb)
        .map_err(|e| e.to_string())?;
    Ok(buf)
}

/// Format of an extracted image XObject (`S10.2`, `S2.6`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    /// Original JPEG stream bytes preserved bit-for-bit.
    Jpeg,
    /// Decoded raster re-encoded as PNG.
    Png,
}

/// An image extracted from a page (`S10.2`, `S2.6`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedImage {
    pub name: String,
    pub format: ImageFormat,
    pub width: usize,
    pub height: usize,
    pub bytes: Vec<u8>,
}

/// Extract all image XObjects from `page`, preserving DCT streams byte-for-byte (`S2.6`, `S10.2`).
#[must_use]
pub fn extract_images(doc: &Document, page: &Page) -> Vec<ExtractedImage> {
    let res = page
        .inherited
        .resources
        .as_ref()
        .and_then(|o| doc.resolve_object(o).or_else(|| Some(o.clone())))
        .and_then(|o| o.as_dict().cloned())
        .map(|d| Resources::from_dict(&d, &|o| doc.resolve_object(o)))
        .unwrap_or_default();

    let mut out = Vec::new();
    for (name, obj) in &res.xobjects {
        let Some(resolved) = doc.resolve_object(obj).or_else(|| Some(obj.clone())) else {
            continue;
        };
        let Object::Stream(stream) = resolved else {
            continue;
        };
        if stream.dict.get("Subtype").and_then(Object::as_name) != Some(b"Image") {
            continue;
        }
        let width = stream
            .dict
            .get("Width")
            .and_then(Object::as_i64)
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(0);
        let height = stream
            .dict
            .get("Height")
            .and_then(Object::as_i64)
            .and_then(|n| usize::try_from(n).ok())
            .unwrap_or(0);
        if width == 0 || height == 0 {
            continue;
        }
        let filters = stream.filters();
        if filters.len() == 1 && filters.first().is_some_and(|f| *f == b"DCTDecode") {
            out.push(ExtractedImage {
                name: name.clone(),
                format: ImageFormat::Jpeg,
                width,
                height,
                bytes: stream.raw.clone(),
            });
            continue;
        }
        let mut notes = Vec::new();
        if let Some(raster) =
            mangle_render::image::decode(&stream, &|o| doc.resolve_object(o), &mut notes)
        {
            let mut img = Image::new(raster.width, raster.height);
            img.pixels = raster.pixels;
            out.push(ExtractedImage {
                name: name.clone(),
                format: ImageFormat::Png,
                width: raster.width,
                height: raster.height,
                bytes: encode_png(&img),
            });
        }
    }
    out
}

/// Input image for [`create_pdf_from_images`] (`S10.3`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageInput {
    /// Raw JPEG bytes passed through unmodified via `/Filter /DCTDecode`.
    Jpeg {
        width: usize,
        height: usize,
        bytes: Vec<u8>,
    },
    /// RGBA pixels stored via `/Filter /FlateDecode` with an `/SMask` when translucent.
    Rgba {
        width: usize,
        height: usize,
        pixels: Vec<u8>,
    },
}

/// Target page sizing when creating a PDF from images (`S10.3`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PageFit {
    /// US Letter (`612 × 792` pt).
    Letter,
    /// ISO A4 (`595 × 842` pt).
    A4,
    /// Match each image's pixel dimensions at 1 pt/px.
    Original,
}

/// Create a valid multi-page PDF from a slice of images, keeping JPEG bytes bit-identical (`S10.3`).
#[must_use]
pub fn create_pdf_from_images(images: &[ImageInput], fit: PageFit) -> Vec<u8> {
    let mut out = Vec::from(b"%PDF-1.7\n%\xe2\xe3\xcf\xd3\n".as_slice());
    let mut offsets = vec![0usize];

    // 1 0 obj: Catalog, 2 0 obj: Pages (written after page objects are allocated).
    offsets.push(0); // placeholder for 1 0 obj
    offsets.push(0); // placeholder for 2 0 obj

    let mut page_nums = Vec::with_capacity(images.len());

    for img in images {
        let (iw, ih) = match img {
            ImageInput::Jpeg { width, height, .. } | ImageInput::Rgba { width, height, .. } => {
                (*width, *height)
            }
        };
        let (pw, ph) = match fit {
            PageFit::Letter => (612.0, 792.0),
            PageFit::A4 => (595.0, 842.0),
            PageFit::Original => (iw.max(1) as f64, ih.max(1) as f64),
        };

        let img_num = match img {
            ImageInput::Jpeg {
                width,
                height,
                bytes,
            } => {
                let num = offsets.len() as u32;
                offsets.push(out.len());
                let header = format!(
                    "{num} 0 obj\n<</Type /XObject /Subtype /Image /Width {width} /Height {height} \
                     /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode /Length {}>>\nstream\n",
                    bytes.len()
                );
                out.extend_from_slice(header.as_bytes());
                out.extend_from_slice(bytes);
                out.extend_from_slice(b"\nendstream\nendobj\n");
                num
            }
            ImageInput::Rgba {
                width,
                height,
                pixels,
            } => {
                let mut rgb = Vec::with_capacity(width * height * 3);
                let mut alpha = Vec::with_capacity(width * height);
                let mut has_alpha = false;
                for px in pixels.chunks_exact(4) {
                    rgb.push(px.first().copied().unwrap_or(0));
                    rgb.push(px.get(1).copied().unwrap_or(0));
                    rgb.push(px.get(2).copied().unwrap_or(0));
                    let a = px.get(3).copied().unwrap_or(255);
                    if a < 255 {
                        has_alpha = true;
                    }
                    alpha.push(a);
                }
                let smask_num = if has_alpha {
                    let s_num = offsets.len() as u32;
                    offsets.push(out.len());
                    let z_alpha = deflate(&alpha, DeflateLevel::Default);
                    let header = format!(
                        "{s_num} 0 obj\n<</Type /XObject /Subtype /Image /Width {width} /Height {height} \
                         /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /FlateDecode /Length {}>>\nstream\n",
                        z_alpha.len()
                    );
                    out.extend_from_slice(header.as_bytes());
                    out.extend_from_slice(&z_alpha);
                    out.extend_from_slice(b"\nendstream\nendobj\n");
                    Some(s_num)
                } else {
                    None
                };

                let num = offsets.len() as u32;
                offsets.push(out.len());
                let z_rgb = deflate(&rgb, DeflateLevel::Default);
                let smask_entry = smask_num
                    .map(|s| format!(" /SMask {s} 0 R"))
                    .unwrap_or_default();
                let header = format!(
                    "{num} 0 obj\n<</Type /XObject /Subtype /Image /Width {width} /Height {height} \
                     /ColorSpace /DeviceRGB /BitsPerComponent 8{smask_entry} /Filter /FlateDecode /Length {}>>\nstream\n",
                    z_rgb.len()
                );
                out.extend_from_slice(header.as_bytes());
                out.extend_from_slice(&z_rgb);
                out.extend_from_slice(b"\nendstream\nendobj\n");
                num
            }
        };

        let content_num = offsets.len() as u32;
        offsets.push(out.len());
        let content_str = format!("q {pw:.2} 0 0 {ph:.2} 0 0 cm /Im0 Do Q");
        let _ = write!(
            out,
            "{content_num} 0 obj\n<</Length {}>>\nstream\n{content_str}\nendstream\nendobj\n",
            content_str.len()
        );

        let page_num = offsets.len() as u32;
        offsets.push(out.len());
        let _ = write!(
            out,
            "{page_num} 0 obj\n<</Type /Page /Parent 2 0 R /MediaBox [0 0 {pw:.2} {ph:.2}] \
             /Contents {content_num} 0 R /Resources <</XObject <</Im0 {img_num} 0 R>>>>>>\nendobj\n"
        );
        page_nums.push(page_num);
    }

    if let Some(slot) = offsets.get_mut(1) {
        *slot = out.len();
    }
    out.extend_from_slice(b"1 0 obj\n<</Type /Catalog /Pages 2 0 R>>\nendobj\n");

    if let Some(slot) = offsets.get_mut(2) {
        *slot = out.len();
    }
    let kids: Vec<String> = page_nums.iter().map(|n| format!("{n} 0 R")).collect();
    let _ = write!(
        out,
        "2 0 obj\n<</Type /Pages /Kids [{}] /Count {}>>\nendobj\n",
        kids.join(" "),
        page_nums.len()
    );

    let start_xref = out.len();
    let _ = write!(out, "xref\n0 {}\n0000000000 65535 f \n", offsets.len());
    for off in offsets.iter().skip(1) {
        let _ = writeln!(out, "{off:010} 00000 n ");
    }
    let _ = write!(
        out,
        "trailer\n<</Size {} /Root 1 0 R>>\nstartxref\n{start_xref}\n%%EOF\n",
        offsets.len()
    );
    out
}

/// Optimization preset (`S10.4`, `S10.5`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptimizePreset {
    /// Lossless cleanup: compress unfiltered streams, deduplicate objects, and GC orphans.
    Lossless,
    /// Small size preset: lossless cleanup plus metadata/thumbnail stripping.
    Small,
}

/// Report returned by [`optimize_document`] (`S10.4`, `S10.5`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OptimizeReport {
    pub bytes: Vec<u8>,
    pub size_before: usize,
    pub size_after: usize,
    pub streams_compressed: usize,
    pub duplicates_merged: usize,
}

/// Optimize and compress `doc` deterministically (`S10.4`, `S10.5`, `S10.7`).
pub fn optimize_document(
    doc: &Document,
    preset: OptimizePreset,
) -> Result<OptimizeReport, SaveError> {
    if doc.info().encryption.encrypted {
        return Err(SaveError::Encrypted);
    }
    let size_before = doc.bytes().len();
    let mut streams_compressed = 0usize;
    let mut duplicates_merged = 0usize;

    // 1. Compress any uncompressed streams that shrink under FlateDecode.
    for num in doc.object_numbers() {
        let r = Ref::new(num, 0);
        let Some(Object::Stream(st)) = doc.object(r) else {
            continue;
        };
        if st.filters().is_empty() && st.raw.len() > 32 {
            let compressed = deflate(&st.raw, DeflateLevel::Default);
            if compressed.len() + 24 < st.raw.len() {
                let mut d = st.dict.clone();
                d.set("Filter", Object::name("FlateDecode"));
                d.set("Length", Object::Int(compressed.len() as i64));
                doc.set(r, Object::Stream(Stream::new(d, compressed)));
                streams_compressed += 1;
            }
        }
    }

    // 2. Deduplicate identical stream objects by SHA-256 hash and rewrite references.
    let mut canonical_streams: BTreeMap<[u8; 32], Ref> = BTreeMap::new();
    let mut aliases: BTreeMap<u32, Ref> = BTreeMap::new();
    for num in doc.object_numbers() {
        let r = Ref::new(num, 0);
        let Some(Object::Stream(st)) = doc.object(r) else {
            continue;
        };
        let decoded = decode_stream(&st);
        let digest = sha256(&decoded.data);
        if let Some(&canon) = canonical_streams.get(&digest) {
            if canon.num != r.num {
                aliases.insert(r.num, canon);
                doc.remove(r);
                duplicates_merged += 1;
            }
        } else {
            canonical_streams.insert(digest, r);
        }
    }

    if !aliases.is_empty() {
        for num in doc.object_numbers() {
            let r = Ref::new(num, 0);
            if aliases.contains_key(&num) {
                continue;
            }
            let Some(obj) = doc.object(r) else {
                continue;
            };
            if let Some(rewritten) = rewrite_refs(&obj, &aliases) {
                doc.set(r, rewritten);
            }
        }
    }

    // 3. Under `Small` preset, strip `/Metadata` and `/Thumb`.
    if preset == OptimizePreset::Small {
        if let Some(cat_ref) = doc.catalog_ref()
            && let Ok(cat_obj) = doc.catalog()
            && let Some(mut cat_dict) = cat_obj.as_dict().cloned()
            && cat_dict.remove("Metadata").is_some()
        {
            doc.set(cat_ref, Object::Dict(cat_dict));
        }
        for num in doc.object_numbers() {
            let r = Ref::new(num, 0);
            if let Some(Object::Dict(mut d)) = doc.object(r)
                && d.remove("Thumb").is_some()
            {
                doc.set(r, Object::Dict(d));
            }
        }
    }

    let saved = doc
        .save(&SaveOptions {
            mode: SaveMode::Full,
            ..SaveOptions::default()
        })
        .map_err(|_| SaveError::ContentsNotIndirect)?;
    let size_after = saved.bytes.len();

    Ok(OptimizeReport {
        bytes: saved.bytes,
        size_before,
        size_after,
        streams_compressed,
        duplicates_merged,
    })
}

fn rewrite_refs(obj: &Object, aliases: &BTreeMap<u32, Ref>) -> Option<Object> {
    match obj {
        Object::Ref(r) => aliases.get(&r.num).copied().map(Object::Ref),
        Object::Array(items) => {
            let mut changed = false;
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                if let Some(rw) = rewrite_refs(item, aliases) {
                    out.push(rw);
                    changed = true;
                } else {
                    out.push(item.clone());
                }
            }
            changed.then_some(Object::Array(out))
        }
        Object::Dict(dict) => {
            let mut changed = false;
            let mut out = Dict::new();
            for (k, v) in dict.iter() {
                if let Some(rw) = rewrite_refs(v, aliases) {
                    out.insert(k.clone(), rw);
                    changed = true;
                } else {
                    out.insert(k.clone(), v.clone());
                }
            }
            changed.then_some(Object::Dict(out))
        }
        Object::Stream(st) => {
            let rw_dict = rewrite_refs(&Object::Dict(st.dict.clone()), aliases)?;
            let Object::Dict(d) = rw_dict else {
                return None;
            };
            Some(Object::Stream(Stream {
                dict: d,
                raw: st.raw.clone(),
                file_offset: None,
                synthetic: true,
            }))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing
    )]

    use mangle_content::Resources;
    use mangle_crypto::sha256;
    use mangle_doc::PageTree;
    use mangle_syntax::object::Object;
    use mangle_syntax::{Document, OpenOptions};

    use super::{
        ImageFormat, ImageInput, OptimizePreset, PageFit, create_pdf_from_images, export_page_jpeg,
        export_page_png, extract_images, optimize_document,
    };

    #[test]
    fn create_pdf_from_images_preserves_jpeg_hash_and_exports_png_and_jpeg() {
        // Encode a 16x16 test JPEG via jpeg_encoder.
        let rgb = vec![200u8; 16 * 16 * 3];
        let mut jpeg_bytes = Vec::new();
        jpeg_encoder::Encoder::new(&mut jpeg_bytes, 90)
            .encode(&rgb, 16, 16, jpeg_encoder::ColorType::Rgb)
            .expect("encodes jpeg");
        let expected_hash = sha256(&jpeg_bytes);

        let rgba = vec![128u8; 8 * 8 * 4];
        let pdf_bytes = create_pdf_from_images(
            &[
                ImageInput::Jpeg {
                    width: 16,
                    height: 16,
                    bytes: jpeg_bytes,
                },
                ImageInput::Rgba {
                    width: 8,
                    height: 8,
                    pixels: rgba,
                },
            ],
            PageFit::Letter,
        );

        let doc = Document::open(pdf_bytes, OpenOptions::default()).expect("opens created pdf");
        let cat = doc.catalog().expect("cat");
        let root = cat.get("Pages").and_then(Object::as_ref_id).expect("root");
        let tree = PageTree::build(&doc, root).expect("tree");
        assert_eq!(tree.len(), 2);

        // Extract image from page 0: JPEG stream SHA-256 is bit-identical!
        let p0 = tree.get(0).expect("p0");
        let extracted0 = extract_images(&doc, p0);
        assert_eq!(extracted0.len(), 1);
        assert_eq!(extracted0[0].format, ImageFormat::Jpeg);
        assert_eq!(sha256(&extracted0[0].bytes), expected_hash);

        // Export page 0 to PNG and JPEG at 144 DPI:
        let res0 = p0
            .inherited
            .resources
            .as_ref()
            .and_then(|o| doc.resolve_object(o).or_else(|| Some(o.clone())))
            .and_then(|o| o.as_dict().cloned())
            .map(|d| Resources::from_dict(&d, &|o| doc.resolve_object(o)))
            .unwrap_or_default();
        let png = export_page_png(&doc, p0, &res0, 144.0);
        assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
        let jpg = export_page_jpeg(&doc, p0, &res0, 144.0, 85).expect("exports jpeg");
        assert!(jpg.starts_with(&[0xFF, 0xD8]));
    }

    #[test]
    fn optimize_deduplicates_streams_compresses_and_is_deterministic() {
        let rgba = vec![42u8; 32 * 32 * 4];
        let pdf = create_pdf_from_images(
            &[
                ImageInput::Rgba {
                    width: 32,
                    height: 32,
                    pixels: rgba.clone(),
                },
                ImageInput::Rgba {
                    width: 32,
                    height: 32,
                    pixels: rgba,
                },
            ],
            PageFit::Letter,
        );
        let doc1 = Document::open(pdf.clone(), OpenOptions::default()).expect("opens 1");
        let rep1 = optimize_document(&doc1, OptimizePreset::Lossless).expect("optimizes 1");
        assert!(rep1.duplicates_merged >= 1);
        assert!(rep1.size_after < rep1.size_before);

        let doc2 = Document::open(pdf, OpenOptions::default()).expect("opens 2");
        let rep2 = optimize_document(&doc2, OptimizePreset::Lossless).expect("optimizes 2");
        assert_eq!(rep1.bytes, rep2.bytes, "optimization is byte-deterministic");
    }
}
