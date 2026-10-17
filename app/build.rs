use std::{env, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let source = "../packaging/linux/io.github.cradiy.Germinal.svg";
    println!("cargo:rerun-if-changed={source}");
    if env::var("CARGO_CFG_TARGET_OS")?.as_str() != "windows" {
        return Ok(());
    }
    let tree = resvg::usvg::Tree::from_data(&fs::read(source)?, &Default::default())?;
    let sizes = [16u32, 24, 32, 48, 64, 128, 256];
    let mut frames = Vec::new();
    for size in sizes {
        let mut pixmap =
            resvg::tiny_skia::Pixmap::new(size, size).ok_or("icon allocation failed")?;
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_scale(
                size as f32 / tree.size().width(),
                size as f32 / tree.size().height(),
            ),
            &mut pixmap.as_mut(),
        );
        let mut rgba = pixmap.take();
        for pixel in rgba.as_chunks_mut::<4>().0 {
            let alpha = u32::from(pixel[3]);
            for channel in &mut pixel[..3] {
                *channel = (u32::from(*channel) * 255 + alpha / 2)
                    .checked_div(alpha)
                    .unwrap_or(0)
                    .min(255) as u8;
            }
        }
        let mut png = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png, size, size);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.write_header()?.write_image_data(&rgba)?;
        }
        frames.push(png);
    }
    let mut ico = vec![0, 0, 1, 0];
    ico.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (size, frame) in sizes.into_iter().zip(&frames) {
        ico.extend_from_slice(&[size as u8, size as u8, 0, 0]);
        ico.extend_from_slice(&1u16.to_le_bytes());
        ico.extend_from_slice(&32u16.to_le_bytes());
        ico.extend_from_slice(&(frame.len() as u32).to_le_bytes());
        ico.extend_from_slice(&offset.to_le_bytes());
        offset += frame.len() as u32;
    }
    for frame in frames {
        ico.extend(frame);
    }
    let path = PathBuf::from(env::var_os("OUT_DIR").ok_or("missing OUT_DIR")?).join("germinal.ico");
    fs::write(&path, ico)?;
    winresource::WindowsResource::new()
        .set_icon(path.to_str().ok_or("non-UTF-8 icon path")?)
        .set("ProductName", "Germinal")
        .set("FileDescription", "Germinal terminal")
        .set("OriginalFilename", "germinal.exe")
        .compile()?;
    Ok(())
}
