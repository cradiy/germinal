use std::{
    collections::HashMap,
    env,
    io::{self, Write},
    thread,
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use unicode_width::UnicodeWidthChar as _;

const IMAGE_ID: u32 = 4_271_903;
const KITTY_CHUNK_SIZE: usize = 4_096;

fn main() {
    if let Err(error) = run() {
        eprintln!("terminal-benchmark: {error}");
        std::process::exit(2);
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let Some(workload) = args.next() else {
        print_usage();
        return Err("missing workload".into());
    };
    if workload == "--help" || workload == "-h" || workload == "help" {
        print_usage();
        return Ok(());
    }

    let options = Options::parse(args)?;
    match workload.as_str() {
        "text" => run_text(&options),
        "image" => run_image(&options),
        "animation" => run_animation(&options),
        _ => {
            print_usage();
            Err(format!("unknown workload: {workload}"))
        }
    }
}

fn print_usage() {
    eprintln!(
        "\
Usage:
  terminal_benchmark text [--mode flood|paced] [--profile ascii|unicode]
      [--lines 250000] [--duration-ms 15000] [--fps 120] [--columns 120]
  terminal_benchmark image [--format rgba|png] [--width 960] [--height 540]
      [--columns 120] [--rows 30] [--hold-ms 10000]
  terminal_benchmark animation [--width 640] [--height 360] [--frames 12]
      [--frame-ms 8] [--columns 120] [--rows 30] [--hold-ms 15000]

The program writes deterministic ANSI or Kitty Graphics Protocol traffic to stdout.
Machine-readable result records are written to stderr with a BENCH_RESULT prefix."
    );
}

struct Options {
    values: HashMap<String, String>,
}

impl Options {
    fn parse(args: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut args = args.peekable();
        let mut values = HashMap::new();
        while let Some(option) = args.next() {
            if !option.starts_with("--") {
                return Err(format!("expected an option, got: {option}"));
            }
            let Some(value) = args.next() else {
                return Err(format!("missing value for {option}"));
            };
            if value.starts_with("--") {
                return Err(format!("missing value for {option}"));
            }
            values.insert(option.trim_start_matches("--").to_string(), value);
        }
        Ok(Self { values })
    }

    fn string(&self, name: &str, default: &str) -> String {
        self.values
            .get(name)
            .cloned()
            .unwrap_or_else(|| default.to_string())
    }

    fn u32(&self, name: &str, default: u32) -> Result<u32, String> {
        let Some(value) = self.values.get(name) else {
            return Ok(default);
        };
        value
            .parse()
            .map_err(|_| format!("--{name} must be a positive integer, got: {value}"))
    }

    fn usize(&self, name: &str, default: usize) -> Result<usize, String> {
        let Some(value) = self.values.get(name) else {
            return Ok(default);
        };
        value
            .parse()
            .map_err(|_| format!("--{name} must be a positive integer, got: {value}"))
    }

    fn ensure_only(&self, allowed: &[&str]) -> Result<(), String> {
        if let Some(unknown) = self
            .values
            .keys()
            .find(|name| !allowed.contains(&name.as_str()))
        {
            Err(format!("unknown option: --{unknown}"))
        } else {
            Ok(())
        }
    }
}

fn run_text(options: &Options) -> Result<(), String> {
    options.ensure_only(&["mode", "profile", "lines", "duration-ms", "fps", "columns"])?;
    let mode = options.string("mode", "flood");
    let profile = options.string("profile", "ascii");
    let columns = options.usize("columns", 120)?.max(24);
    match mode.as_str() {
        "flood" => {
            let lines = options.u32("lines", 250_000)?.max(1);
            text_flood(lines, columns, &profile)
        }
        "paced" => {
            let duration_ms = options.u32("duration-ms", 15_000)?.max(1);
            let fps = options.u32("fps", 120)?.clamp(1, 1_000);
            text_paced(
                Duration::from_millis(duration_ms.into()),
                fps,
                columns,
                &profile,
            )
        }
        _ => Err(format!("--mode must be flood or paced, got: {mode}")),
    }
}

fn text_flood(lines: u32, columns: usize, profile: &str) -> Result<(), String> {
    validate_text_profile(profile)?;
    let mut stdout = io::stdout().lock();
    stdout
        .write_all(b"\x1b[?25l\x1b[2J\x1b[H")
        .map_err(io_error)?;
    let started = Instant::now();
    let mut bytes = 0_u64;
    let mut batch = Vec::with_capacity((columns + 64) * 256);

    for line in 0..lines {
        append_text_line(&mut batch, line as u64, columns, profile);
        if batch.len() >= (columns + 64) * 256 {
            stdout.write_all(&batch).map_err(io_error)?;
            bytes += batch.len() as u64;
            batch.clear();
        }
    }
    if !batch.is_empty() {
        stdout.write_all(&batch).map_err(io_error)?;
        bytes += batch.len() as u64;
    }
    stdout.write_all(b"\x1b[0m\x1b[?25h").map_err(io_error)?;
    stdout.flush().map_err(io_error)?;

    let elapsed = started.elapsed();
    result_record(&format!(
        "workload=text mode=flood profile={profile} lines={lines} bytes={bytes} elapsed_ms={:.3} producer_mib_s={:.3}",
        elapsed.as_secs_f64() * 1_000.0,
        bytes as f64 / 1_048_576.0 / elapsed.as_secs_f64().max(0.000_001),
    ));
    Ok(())
}

fn text_paced(duration: Duration, fps: u32, columns: usize, profile: &str) -> Result<(), String> {
    validate_text_profile(profile)?;
    let mut stdout = io::stdout().lock();
    stdout
        .write_all(b"\x1b[?25l\x1b[2J\x1b[H")
        .map_err(io_error)?;
    let frame_interval = Duration::from_secs_f64(1.0 / f64::from(fps));
    let started = Instant::now();
    let deadline = started + duration;
    let mut next_frame = started;
    let mut frame = 0_u64;
    let mut producer_deadline_misses = 0_u64;
    let mut bytes = 0_u64;
    let mut line = Vec::with_capacity(columns + 64);

    while Instant::now() < deadline {
        line.clear();
        append_text_line(&mut line, frame, columns, profile);
        stdout.write_all(&line).map_err(io_error)?;
        stdout.flush().map_err(io_error)?;
        bytes += line.len() as u64;
        frame += 1;
        next_frame += frame_interval;
        let now = Instant::now();
        if now < next_frame {
            thread::sleep(next_frame - now);
        } else {
            producer_deadline_misses += 1;
        }
    }
    stdout.write_all(b"\x1b[0m\x1b[?25h").map_err(io_error)?;
    stdout.flush().map_err(io_error)?;

    result_record(&format!(
        "workload=text mode=paced profile={profile} requested_fps={fps} frames={frame} bytes={bytes} elapsed_ms={:.3} producer_deadline_misses={producer_deadline_misses}",
        started.elapsed().as_secs_f64() * 1_000.0,
    ));
    Ok(())
}

fn validate_text_profile(profile: &str) -> Result<(), String> {
    if matches!(profile, "ascii" | "unicode") {
        Ok(())
    } else {
        Err(format!(
            "--profile must be ascii or unicode, got: {profile}"
        ))
    }
}

fn append_text_line(buffer: &mut Vec<u8>, sequence: u64, columns: usize, profile: &str) {
    let red = 80 + sequence % 160;
    let green = 255 - sequence % 120;
    let blue = 120 + sequence % 120;
    buffer.extend_from_slice(format!("\x1b[38;2;{red};{green};{blue}m{sequence:08} ").as_bytes());
    let prefix_columns = 9;
    let pattern = if profile == "unicode" {
        "Germinal Kitty Zellij 终端性能 │─┼ αβγ → "
    } else {
        "Germinal Kitty Zellij | terminal rendering benchmark | "
    };
    let mut visible = prefix_columns;
    while visible < columns {
        for character in pattern.chars() {
            if visible >= columns {
                break;
            }
            let mut encoded = [0_u8; 4];
            let width = character.width().unwrap_or(0);
            if visible + width > columns {
                break;
            }
            buffer.extend_from_slice(character.encode_utf8(&mut encoded).as_bytes());
            visible += width;
        }
    }
    buffer.extend_from_slice(b"\x1b[0m\r\n");
}

fn run_image(options: &Options) -> Result<(), String> {
    options.ensure_only(&["format", "width", "height", "columns", "rows", "hold-ms"])?;
    let format = options.string("format", "rgba");
    let width = options.u32("width", 960)?.clamp(1, 4_096);
    let height = options.u32("height", 540)?.clamp(1, 4_096);
    let columns = options.u32("columns", 120)?.max(1);
    let rows = options.u32("rows", 30)?.max(1);
    let hold_ms = options.u32("hold-ms", 10_000)?;
    let rgba = image_pixels(width, height, 0);
    let (kitty_format, payload) = encode_image_payload(&format, width, height, &rgba)?;
    let encoded = STANDARD.encode(&payload);
    let mut stdout = io::stdout().lock();
    stdout
        .write_all(b"\x1b[?25l\x1b[2J\x1b[H")
        .map_err(io_error)?;
    let started = Instant::now();
    write_kitty_payload(
        &mut stdout,
        &format!(
            "a=T,f={kitty_format},s={width},v={height},i={IMAGE_ID},c={columns},r={rows},C=1,q=2"
        ),
        "",
        &encoded,
    )?;
    stdout.flush().map_err(io_error)?;
    let transmit = started.elapsed();
    let result = format!(
        "workload=image format={format} width={width} height={height} rgba_bytes={} payload_bytes={} encoded_bytes={} transmit_ms={:.3} hold_ms={hold_ms}",
        rgba.len(),
        payload.len(),
        encoded.len(),
        transmit.as_secs_f64() * 1_000.0,
    );
    thread::sleep(Duration::from_millis(hold_ms.into()));
    delete_benchmark_image(&mut stdout)?;
    result_record(&result);
    Ok(())
}

fn encode_image_payload(
    format: &str,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Result<(u32, Vec<u8>), String> {
    match format {
        "rgba" => Ok((32, rgba.to_vec())),
        "png" => {
            let mut png = Vec::new();
            let mut encoder = png::Encoder::new(&mut png, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().map_err(|error| error.to_string())?;
            writer
                .write_image_data(rgba)
                .map_err(|error| error.to_string())?;
            writer.finish().map_err(|error| error.to_string())?;
            Ok((100, png))
        }
        _ => Err(format!("--format must be rgba or png, got: {format}")),
    }
}

fn run_animation(options: &Options) -> Result<(), String> {
    options.ensure_only(&[
        "width", "height", "frames", "frame-ms", "columns", "rows", "hold-ms",
    ])?;
    let width = options.u32("width", 640)?.clamp(1, 2_048);
    let height = options.u32("height", 360)?.clamp(1, 2_048);
    let frames = options.u32("frames", 12)?.clamp(2, 120);
    let frame_ms = options.u32("frame-ms", 8)?.clamp(1, 60_000);
    let columns = options.u32("columns", 120)?.max(1);
    let rows = options.u32("rows", 30)?.max(1);
    let hold_ms = options.u32("hold-ms", 15_000)?;
    let mut stdout = io::stdout().lock();
    stdout
        .write_all(b"\x1b[?25l\x1b[2J\x1b[H")
        .map_err(io_error)?;
    let started = Instant::now();
    let mut rgba_bytes = 0_usize;
    let mut encoded_bytes = 0_usize;

    for frame in 0..frames {
        let rgba = image_pixels(width, height, frame);
        let encoded = STANDARD.encode(&rgba);
        rgba_bytes += rgba.len();
        encoded_bytes += encoded.len();
        if frame == 0 {
            write_kitty_payload(
                &mut stdout,
                &format!("a=T,f=32,s={width},v={height},i={IMAGE_ID},c={columns},r={rows},C=1,q=2"),
                "",
                &encoded,
            )?;
        } else {
            write_kitty_payload(
                &mut stdout,
                &format!("a=f,f=32,s={width},v={height},i={IMAGE_ID},z={frame_ms},X=1,q=2"),
                "a=f,",
                &encoded,
            )?;
        }
    }
    write!(
        stdout,
        "\x1b_Ga=a,i={IMAGE_ID},r=1,z={frame_ms},q=2\x1b\\\x1b_Ga=a,i={IMAGE_ID},s=3,v=1,q=2\x1b\\"
    )
    .map_err(io_error)?;
    stdout.flush().map_err(io_error)?;
    let transmit = started.elapsed();
    let result = format!(
        "workload=animation width={width} height={height} frames={frames} frame_ms={frame_ms} target_fps={:.3} rgba_bytes={rgba_bytes} encoded_bytes={encoded_bytes} transmit_ms={:.3} hold_ms={hold_ms}",
        1_000.0 / f64::from(frame_ms),
        transmit.as_secs_f64() * 1_000.0,
    );
    thread::sleep(Duration::from_millis(hold_ms.into()));
    delete_benchmark_image(&mut stdout)?;
    result_record(&result);
    Ok(())
}

fn image_pixels(width: u32, height: u32, frame: u32) -> Vec<u8> {
    let mut pixels = Vec::with_capacity(width as usize * height as usize * 4);
    let bar_width = (width / 8).max(1);
    let travel = width.saturating_sub(bar_width).max(1);
    let bar_left = (frame.wrapping_mul(width / 12 + 1)) % travel;
    for y in 0..height {
        for x in 0..width {
            let checker = ((x / 32) + (y / 32)) % 2;
            let in_bar = x >= bar_left && x < bar_left + bar_width;
            let red = if in_bar {
                255
            } else {
                ((x * 255) / width.max(1)) as u8
            };
            let green = if in_bar {
                245
            } else {
                ((y * 255) / height.max(1)) as u8
            };
            let blue = if checker == 0 { 48 } else { 112 };
            pixels.extend_from_slice(&[red, green, blue, 255]);
        }
    }
    pixels
}

fn write_kitty_payload(
    stdout: &mut impl Write,
    first_control: &str,
    continuation_prefix: &str,
    payload: &str,
) -> Result<(), String> {
    let chunks = payload.as_bytes().chunks(KITTY_CHUNK_SIZE);
    for (index, chunk) in chunks.enumerate() {
        let more = usize::from((index + 1) * KITTY_CHUNK_SIZE < payload.len());
        if index == 0 {
            write!(stdout, "\x1b_G{first_control},m={more};").map_err(io_error)?;
        } else {
            write!(stdout, "\x1b_G{continuation_prefix}m={more},q=2;").map_err(io_error)?;
        }
        stdout.write_all(chunk).map_err(io_error)?;
        stdout.write_all(b"\x1b\\").map_err(io_error)?;
    }
    Ok(())
}

fn delete_benchmark_image(stdout: &mut impl Write) -> Result<(), String> {
    write!(
        stdout,
        "\x1b_Ga=d,d=i,i={IMAGE_ID},q=2\x1b\\\x1b[0m\x1b[?25h\x1b[2J\x1b[H"
    )
    .map_err(io_error)?;
    stdout.flush().map_err(io_error)
}

fn result_record(fields: &str) {
    eprintln!("BENCH_RESULT {fields}");
}

fn io_error(error: io::Error) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kitty_payload_chunks_large_data() {
        let payload = "A".repeat(KITTY_CHUNK_SIZE + 1);
        let mut output = Vec::new();
        write_kitty_payload(&mut output, "a=T,q=2", "", &payload).unwrap();
        let output = String::from_utf8(output).unwrap();
        assert!(output.starts_with("\x1b_Ga=T,q=2,m=1;"));
        assert!(output.contains("\x1b\\\x1b_Gm=0,q=2;"));
    }

    #[test]
    fn generated_image_has_expected_rgba_size() {
        assert_eq!(image_pixels(12, 7, 0).len(), 12 * 7 * 4);
    }

    #[test]
    fn png_payload_has_png_signature() {
        let rgba = image_pixels(12, 7, 0);
        let (kitty_format, payload) = encode_image_payload("png", 12, 7, &rgba).unwrap();
        assert_eq!(kitty_format, 100);
        assert_eq!(&payload[..8], b"\x89PNG\r\n\x1a\n");
    }
}
