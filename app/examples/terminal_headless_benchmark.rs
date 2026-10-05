use std::{fmt::Write as _, hint::black_box, time::Instant};

use clap::{Parser, ValueEnum};
use germinal_infra::{
    pty_host::alacritty_terminal_store::{AlacrittyTermSize, AlacrittyTerminalStore},
    rendering::pty_surface::{
        crossfont_glyph_atlas::WgpuCrossfontGlyphAtlasBuilder,
        glyph_atlas_frame::WgpuTerminalGlyphAtlasFrameBuilder,
    },
};
use germinal_ports::{
    pty_host::snapshot::TerminalSnapshotProvider, rendering::render_target_id::RenderTargetId,
    seq::Seq,
};

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Profile {
    Ascii,
    UnicodeRepeated,
    UnicodeUnique,
    Links,
}

#[derive(Parser)]
#[command(about = "Measure terminal CPU stages without a window, GPU, or PTY process")]
struct Args {
    #[arg(long, value_enum, default_value = "unicode-repeated")]
    profile: Profile,
    #[arg(long, default_value_t = 1000, value_parser = clap::value_parser!(u32).range(1..=1000000))]
    frames: u32,
    #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u32).range(1..=1000000))]
    warmup: u32,
    #[arg(long, default_value_t = 3, value_parser = clap::value_parser!(u32).range(1..=100))]
    repeats: u32,
    #[arg(long, default_value_t = 142, value_parser = clap::value_parser!(u16).range(8..=1000))]
    columns: u16,
    #[arg(long, default_value_t = 40, value_parser = clap::value_parser!(u16).range(2..=1000))]
    rows: u16,
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u16).range(1..=1000))]
    lines_per_frame: u16,
    #[arg(long, default_value = "JetBrainsMono Nerd Font")]
    font: String,
    #[arg(long)]
    ligatures: bool,
}

fn payload(profile: Profile, index: u32) -> Vec<u8> {
    let text = match profile {
        Profile::Ascii => "Germinal terminal text 0123456789".to_owned(),
        Profile::UnicodeRepeated => "Germinal 终端性能 αβγ ─┼ →".to_owned(),
        Profile::UnicodeUnique => {
            // Fixed glyph vocabulary, changing ordering: exercise unique runs without
            // confusing text-collection cost with continual font-cache growth.
            let alphabet: Vec<_> = "终端性能中文测试字符渲染快照数据优化".chars().collect();
            let mut state = index.wrapping_add(1);
            (0..24)
                .map(|_| {
                    state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                    alphabet[(state >> 16) as usize % alphabet.len()]
                })
                .collect()
        }
        Profile::Links => format!(
            "https://example.com/path/{index} \x1b]8;;file:///tmp/source.rs\x1b\\source.rs:12\x1b]8;;\x1b\\"
        ),
    };
    format!("\x1b[32m{index:08}\x1b[0m {text}\r\n").into_bytes()
}

fn run(args: &Args) -> Result<(), Box<dyn std::error::Error>> {
    let target = RenderTargetId::new(1);
    // A bounded reusable corpus keeps payload generation outside timed sections.
    let corpus: Vec<_> = (0..4096)
        .map(|index| payload(args.profile, index))
        .collect();
    println!(
        "profile,repetition,frames,lines_per_frame,parse_us,snapshot_us,links_us,atlas_us,atlas_hits,content_hits,bytes"
    );
    for repetition in 1..=args.repeats {
        let store = AlacrittyTerminalStore::with_size_and_scrollback_history(
            AlacrittyTermSize::new(args.columns.into(), args.rows.into()),
            10000,
        );
        let atlas = WgpuTerminalGlyphAtlasFrameBuilder::with_crossfont_builder(
            WgpuCrossfontGlyphAtlasBuilder::new(args.font.clone(), 20.0)?
                .with_ligatures(args.ligatures),
        );
        let mut totals = [0_u128; 4];
        let mut atlas_hits = 0;
        let mut content_hits = 0;
        let mut bytes = 0_usize;
        let mut line = 0_usize;
        let mut chunk = Vec::new();
        for frame in 0..args.warmup + args.frames {
            chunk.clear();
            for _ in 0..args.lines_per_frame {
                chunk.extend_from_slice(&corpus[line % corpus.len()]);
                line += 1;
            }
            let seq = Seq::new(u64::from(frame) + 1);
            let start = Instant::now();
            black_box(store.apply_bytes(target, seq, black_box(&chunk)));
            let parsed = Instant::now();
            let snapshot = store
                .render_surface_snapshot_of(target)
                .ok_or("missing snapshot")?;
            black_box(&snapshot);
            let snapped = Instant::now();
            black_box(store.visible_hyperlinks(target));
            let linked = Instant::now();
            let glyphs = atlas.build(black_box(&snapshot));
            black_box(&glyphs);
            let prepared = Instant::now();
            if frame >= args.warmup {
                for (total, duration) in totals.iter_mut().zip([
                    parsed - start,
                    snapped - parsed,
                    linked - snapped,
                    prepared - linked,
                ]) {
                    *total += duration.as_nanos();
                }
                atlas_hits += u32::from(glyphs.cache_hit);
                content_hits += u32::from(glyphs.content_cache_hit);
                bytes += chunk.len();
            }
            store.clear_damage_up_to(target, seq);
        }
        let mut output = format!(
            "{:?},{repetition},{},{},",
            args.profile, args.frames, args.lines_per_frame
        );
        for total in totals {
            write!(
                output,
                "{:.3},",
                total as f64 / f64::from(args.frames) / 1000.0
            )?;
        }
        println!("{output}{atlas_hits},{content_hits},{bytes}");
    }
    Ok(())
}

fn main() {
    if let Err(error) = run(&Args::parse()) {
        eprintln!("terminal-headless-benchmark: {error}");
        std::process::exit(1);
    }
}
