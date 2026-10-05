//! The bound on an image's size is asked **before** anything is allocated, and this measures it.
//!
//! The check lives in an integration test rather than in the unit tests of `image.rs` for a
//! reason that is not tidiness: it runs a child process, and Gate 0.4 allows process spawning in
//! test code but not in a product crate's source. Keeping it here means the rule stays as it is
//! rather than being widened to suit where the test happened to sit.

#![forbid(unsafe_code)]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::process::Command;

use mangle_render::image::decode;
use mangle_syntax::{Dict, Object, Stream};

/// This process's peak virtual size in bytes, which is what an allocation shows up in.
///
/// `VmPeak` is the high-water mark rather than the current size, so a decode that allocated and
/// then freed is still caught by it.
fn peak() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .unwrap_or_default()
        .lines()
        .find_map(|l| l.strip_prefix("VmPeak:"))
        .and_then(|v| v.split_whitespace().next())
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}

/// The mask claims 34862 by 4332 pixels — 151 million samples — and its stream inflates to
/// 400 MB, so a renderer that decoded the stream first and refused afterwards grows its address
/// space by 400 MB doing it. Asserting only that the mask was *refused* would pass equally well
/// for that renderer, and the allocation is the whole of what is at stake, so the assertion is
/// on a process's own peak virtual size.
///
/// It runs in a child because the parent has to build the bomb, and a process that has held
/// 400 MB of zeroes has already reached the peak being measured. The child is this same test
/// binary with one environment variable set, so there is no second program to keep in step.
#[test]
fn a_mask_above_the_bound_does_not_grow_the_address_space() {
    let bomb = std::env::temp_dir().join("mangle-mask-bomb.bin");
    if let Ok(path) = std::env::var("MANGLE_MASK_BOMB") {
        // The child: build the mask from the file and decode it, reporting the growth.
        let packed = std::fs::read(&path).expect("the bomb the parent wrote");
        let mut mask_dict = Dict::new();
        mask_dict.set("Width", Object::Int(34862));
        mask_dict.set("Height", Object::Int(4332));
        mask_dict.set("BitsPerComponent", Object::Int(1));
        mask_dict.set("ColorSpace", Object::name("DeviceGray"));
        mask_dict.set("Filter", Object::name("FlateDecode"));
        let mask = Stream::new(mask_dict, packed);

        let mut dict = Dict::new();
        dict.set("Width", Object::Int(2));
        dict.set("Height", Object::Int(2));
        dict.set("BitsPerComponent", Object::Int(8));
        dict.set("ColorSpace", Object::name("DeviceRGB"));
        dict.set("SMask", Object::Stream(mask));
        let image = Stream::new(dict, vec![255; 24]);

        let before = peak();
        let mut notes = Vec::new();
        let raster = decode(&image, &|o| Some(o.clone()), &mut notes).expect("the image decodes");
        let grew = peak().saturating_sub(before);
        println!(
            "CHILD grew {grew} mask {:?} said {:?}",
            raster.soft_mask.as_ref().map(|m| m.width),
            notes.join("; ")
        );
        return;
    }

    // The parent: 400 MB of zeroes, deflated, handed over and dropped.
    let zeros = vec![0u8; 400 * 1024 * 1024];
    let packed = mangle_filters::deflate(&zeros, mangle_filters::DeflateLevel::Default);
    assert!(
        packed.len() < 4 * 1024 * 1024,
        "400 MB of zeroes must compress to something a file could carry: {} bytes",
        packed.len()
    );
    drop(zeros);
    std::fs::write(&bomb, &packed).expect("write the bomb");
    let out = Command::new(std::env::current_exe().expect("this binary"))
        .args([
            "--exact",
            "a_mask_above_the_bound_does_not_grow_the_address_space",
            "--nocapture",
        ])
        .env("MANGLE_MASK_BOMB", &bomb)
        .output()
        .expect("the child runs");
    let child = String::from_utf8_lossy(&out.stdout);
    let grew = child
        .lines()
        .find_map(|l| l.strip_prefix("CHILD grew "))
        .and_then(|v| v.split_whitespace().next())
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or_else(|| panic!("the child said nothing about its growth:\n{child}"));
    assert!(
        child.contains("soft mask"),
        "the mask is refused, and by name: {child}"
    );
    assert!(
        grew < 64 * 1024,
        "a refused mask grew the address space by {grew} bytes, so the bound was asked after the \
         stream was decoded rather than before"
    );
    let _ = std::fs::remove_file(&bomb);
}
