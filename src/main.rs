use quick_error::ResultExt;
use std::path::PathBuf;

use anyhow::{Result};
use clap::{Parser, Subcommand, ValueEnum};

mod hud;
mod pipeline;
mod telemetry;

use hud::elements::HudElements;
use pipeline::{OutputMode, RenderConfig};
use telemetry::extractor::extract_metadata;
use telemetry::parser::parse_telemetry;

// ═══════════════════════════════════════════════════════════════════════════════
// CLI
// ═══════════════════════════════════════════════════════════════════════════════

#[derive(Parser, Debug)]
#[command(
    name       = "dashcam-hud",
    version,
    about      = "Generate a videogame-style HUD overlay from dashcam telemetry",
    long_about = r#"
dashcam-hud — render a transparent telemetry HUD and composite it over footage.

── WORKFLOW OPTIONS ────────────────────────────────────────────────────────────

  Option A — two-step (HUD as separate overlay file):
    dashcam-hud render clip.MP4 -o hud.mov        # ProRes+alpha overlay
    ffmpeg -i clip.MP4 -i hud.mov \
      -filter_complex "[0:v][1:v]overlay=0:0:format=auto" \
      -c:a copy final.mp4

  Option B — one-step compose (HUD burned into footage directly):
    dashcam-hud compose clip.MP4 final.mp4

  Option C — PNG frames (manual compositing in DaVinci/Premiere):
    dashcam-hud render clip.MP4 -o frames/

── ALPHA-PRESERVING FORMATS ───────────────────────────────────────────────────
  .mov   ProRes 4444   best quality, large file, works in every NLE
  .webm  VP9 alpha     open format, smaller, good for web
  .mkv   FFV1          lossless, very large
  .mp4   ✗ NO ALPHA   background will be black — use compose or .mov instead

── HUD ELEMENT FLAGS ──────────────────────────────────────────────────────────
  --preset full|default|minimal|none    pick a starting point
  --no-speedometer / --speedometer      toggle individual widgets
  --no-gforce      / --gforce
  --no-compass     / --compass
  --no-gps-tag     / --gps-tag
  --no-minimap     / --minimap

  Example — compass + GPS only:
    dashcam-hud compose clip.MP4 out.mp4 --preset none --compass --gps-tag
"#,
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Render a standalone HUD overlay video or PNG frames
    Render(RenderArgs),

    /// Burn the HUD directly onto footage (no intermediate file, no black bg)
    Compose(ComposeArgs),

    /// Print raw telemetry extracted from a video (debug)
    Inspect {
        /// Dashcam video to inspect
        input: PathBuf,
        /// Number of frames to print
        #[arg(short, long, default_value_t = 10)]
        count: usize,
    },
}

// ─── Preset ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, ValueEnum, Default)]
enum HudPreset {
    /// Speedometer + compass + map
    #[default]
    Default,
    /// All five widgets
    Full,
    /// Speedometer only
    Minimal,
    /// Nothing — opt in with individual flags
    None,
}

impl HudPreset {
    fn elements(&self) -> HudElements {
        match self {
            HudPreset::Default => HudElements::default_view(),
            HudPreset::Full    => HudElements::full(),
            HudPreset::Minimal => HudElements::minimal(),
            HudPreset::None    => HudElements::none(),
        }
    }
}

// ─── Shared HUD element overrides ────────────────────────────────────────────

#[derive(Parser, Debug, Clone)]
struct HudFlags {
    /// Starting preset (default = speed+compass+map)
    #[arg(long, value_enum, default_value_t = HudPreset::Default)]
    preset: HudPreset,

    /// Enable speedometer (overrides preset)
    #[arg(long = "speedometer", overrides_with = "no_speedometer")]
    speedometer: bool,
    /// Disable speedometer (overrides preset)
    #[arg(long = "no-speedometer", overrides_with = "speedometer")]
    no_speedometer: bool,

    /// Enable G-force radar (overrides preset)
    #[arg(long = "gforce", overrides_with = "no_gforce")]
    gforce: bool,
    /// Disable G-force radar (overrides preset)
    #[arg(long = "no-gforce", overrides_with = "gforce")]
    no_gforce: bool,

    /// Enable compass bar (overrides preset)
    #[arg(long = "compass", overrides_with = "no_compass")]
    compass: bool,
    /// Disable compass bar (overrides preset)
    #[arg(long = "no-compass", overrides_with = "compass")]
    no_compass: bool,

    /// Enable GPS coordinates tag (overrides preset)
    #[arg(long = "gps-tag", overrides_with = "no_gps_tag")]
    gps_tag: bool,
    /// Disable GPS coordinates tag (overrides preset)
    #[arg(long = "no-gps-tag", overrides_with = "gps_tag")]
    no_gps_tag: bool,

    /// Enable mini-map (overrides preset)
    #[arg(long = "minimap", overrides_with = "no_minimap")]
    minimap: bool,
    /// Disable mini-map (overrides preset)
    #[arg(long = "no-minimap", overrides_with = "minimap")]
    no_minimap: bool,
}

impl HudFlags {
    fn resolve(&self) -> HudElements {
        let mut e = self.preset.elements();
        // Each pair: if the positive flag was explicitly set, force on;
        // if the negative flag was explicitly set, force off.
        if self.speedometer    { e.speedometer = true;  }
        if self.no_speedometer { e.speedometer = false; }
        if self.gforce         { e.gforce      = true;  }
        if self.no_gforce      { e.gforce      = false; }
        if self.compass        { e.compass     = true;  }
        if self.no_compass     { e.compass     = false; }
        if self.gps_tag        { e.gps_tag     = true;  }
        if self.no_gps_tag     { e.gps_tag     = false; }
        if self.minimap        { e.minimap     = true;  }
        if self.no_minimap     { e.minimap     = false; }
        e
    }
}

// ─── Shared render tuning args ────────────────────────────────────────────────

#[derive(Parser, Debug, Clone)]
struct RenderTuning {
    /// Canvas width in pixels
    #[arg(long, default_value_t = 3840)]
    width: u32,

    /// Canvas height in pixels
    #[arg(long, default_value_t = 2160)]
    height: u32,

    /// Output frame rate (must match your dashcam footage)
    #[arg(long, default_value_t = 30)]
    fps: u32,

    /// Maximum speed shown on speedometer (km/h)
    #[arg(long, default_value_t = 120.0)]
    max_speed: f64,

    /// Render threads (0 = all CPUs)
    #[arg(long, default_value_t = 0)]
    threads: usize,

    /// Sync offset in seconds. Positive = HUD data advances (fixes HUD lagging
    /// behind the dashcam speed readout). Negative = HUD data retreats.
    /// Tip: run `inspect` to see timestamps, then adjust until speeds match.
    #[arg(long, default_value_t = 0.0, allow_negative_numbers = true)]
    sync_offset: f64,
}

// ─── render subcommand ────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
struct RenderArgs {
    /// Dashcam video file
    #[arg(value_name = "VIDEO")]
    input: PathBuf,

    /// Output: directory → PNG frames; .mov/.webm/.mkv → alpha video; .mp4 → no alpha
    #[arg(short, long, default_value = "frames")]
    output: PathBuf,

    /// Also write PNG frames when output is a video file
    #[arg(long)]
    save_frames: bool,

    /// Directory to write PNG frames into (with --save-frames)
    #[arg(long, default_value = "frames")]
    frames_dir: PathBuf,

    #[command(flatten)]
    tuning: RenderTuning,

    #[command(flatten)]
    hud: HudFlags,
}

// ─── compose subcommand ───────────────────────────────────────────────────────

#[derive(Parser, Debug)]
struct ComposeArgs {
    /// Dashcam video file (source footage + telemetry)
    #[arg(value_name = "SOURCE")]
    input: PathBuf,

    /// Output video file (HUD burned onto footage)
    #[arg(value_name = "OUTPUT")]
    output: PathBuf,

    #[command(flatten)]
    tuning: RenderTuning,

    #[command(flatten)]
    hud: HudFlags,
}

// ═══════════════════════════════════════════════════════════════════════════════
// ENTRY POINT
// ═══════════════════════════════════════════════════════════════════════════════

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Inspect { input, count } => cmd_inspect(&input, count),
        Commands::Render(args)             => cmd_render(args),
        Commands::Compose(args)            => cmd_compose(args),
    }
}

// ─── inspect ─────────────────────────────────────────────────────────────────

fn cmd_inspect(input: &PathBuf, count: usize) -> Result<()> {
    println!("Extracting telemetry from {}…", input.display());
    let json   = extract_metadata(input).context("exiftool extraction failed").expect("exiftool extraction failed");
    let frames = parse_telemetry(&json);
    println!("Total telemetry frames: {}\n", frames.len());

    for frame in frames.iter().take(count) {
        println!("── {} ──", frame.doc_id);
        match &frame.gps {
            Some(g) => println!(
                "  GPS  lat={:.6}  lon={:.6}  speed={:.1}km/h  hdg={:.1}°  ts={}",
                g.latitude, g.longitude, g.speed.unwrap_or(0.0), g.heading.unwrap_or(0.0), g.timestamp
            ),
            None => println!("  GPS  (none)"),
        }
        match &frame.accel {
            Some(a) => println!("  Accel  x={:+.3}  y={:+.3}  z={:+.3}", a.x, a.y, a.z),
            None    => println!("  Accel  (none)"),
        }
        println!();
    }
    if frames.len() > count {
        println!("… and {} more frames.", frames.len() - count);
    }
    Ok(())
}

// ─── render ──────────────────────────────────────────────────────────────────

fn cmd_render(args: RenderArgs) -> Result<()> {
    configure_threads(args.tuning.threads);

    let elements = args.hud.resolve();
    print_header(&args.input, &args.output, elements);

    let frames = load_telemetry(&args.input)?;

    let output_mode = {
        let ext = args.output.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        let is_video = matches!(ext.as_str(), "mp4" | "mov" | "mkv" | "webm");
        if is_video && args.save_frames {
            OutputMode::Both { frames_dir: args.frames_dir.clone(), video: args.output.clone() }
        } else if is_video {
            OutputMode::FfmpegPipe(args.output.clone())
        } else {
            OutputMode::Frames(args.output.clone())
        }
    };

    let cfg = build_config(&args.tuning, elements, output_mode);
    print_render_summary(&frames, &cfg);
    pipeline::render(&frames, &cfg)?;
    println!("\n✓ Output: {}", args.output.display());
    Ok(())
}

// ─── compose ─────────────────────────────────────────────────────────────────

fn cmd_compose(args: ComposeArgs) -> Result<()> {
    configure_threads(args.tuning.threads);

    let elements = args.hud.resolve();
    print_header(&args.input, &args.output, elements);
    println!("  Mode  : compose (HUD burned directly onto footage)");

    let frames = load_telemetry(&args.input)?;

    let cfg = build_config(
        &args.tuning,
        elements,
        OutputMode::ComposeWithSource {
            source: args.input.clone(),
            output: args.output.clone(),
        },
    );
    print_render_summary(&frames, &cfg);
    pipeline::render(&frames, &cfg)?;
    println!("\n✓ Output: {}", args.output.display());
    Ok(())
}

// ─── Shared helpers ───────────────────────────────────────────────────────────

fn configure_threads(threads: usize) {
    let n = if threads == 0 {
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
    } else {
        threads
    };
    rayon::ThreadPoolBuilder::new().num_threads(n).build_global().ok();
    println!("dashcam-hud  ·  {} render threads", n);
}

fn load_telemetry(input: &PathBuf) -> Result<Vec<telemetry::vantrue_frames::TelemetryFrame>> {
    println!("Extracting telemetry from {}…", input.display());
    let json   = extract_metadata(input).context("exiftool extraction failed").expect("exiftool extraction failed");
    let frames = parse_telemetry(&json);
    println!("Found {} telemetry frames  (1 frame = 1 second of video)", frames.len());
    if frames.len() < 2 {
        anyhow::bail!("Not enough telemetry frames to render (found {}).", frames.len());
    }
    Ok(frames)
}

fn build_config(tuning: &RenderTuning, elements: HudElements, output: OutputMode) -> RenderConfig {
    RenderConfig {
        width:           tuning.width,
        height:          tuning.height,
        fps:             tuning.fps,
        max_speed:       tuning.max_speed,
        elements,
        output,
        sync_offset_s:   tuning.sync_offset,
    }
}

fn print_header(input: &PathBuf, output: &PathBuf, e: HudElements) {
    println!();
    println!("  Input : {}", input.display());
    println!("  Output: {}", output.display());
    println!("  HUD   : {}{}{}{}{}",
             if e.speedometer { "speed " }  else { "" },
             if e.gforce      { "gforce " } else { "" },
             if e.compass     { "compass " }else { "" },
             if e.gps_tag     { "gps " }    else { "" },
             if e.minimap     { "map" }     else { "" },
    );
}

fn print_render_summary(
    frames: &[telemetry::vantrue_frames::TelemetryFrame],
    cfg: &RenderConfig,
) {
    // 1 TelemetryFrame = 1 second of video → total output frames = (n-1) * fps
    let total = (frames.len() - 1) * cfg.fps as usize;
    let dur   = (frames.len() - 1) as f64;
    println!(
        "\nRendering {} frames  ({:.0}s of telemetry @ {}fps)…\n",
        total, dur, cfg.fps
    );
}