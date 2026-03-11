///////////////////////////////////////////////////////////////////////////////
//  pipeline.rs  —  Parallel render pipeline
//
//  SYNC MODEL
//  ══════════
//  Each TelemetryFrame covers exactly one second of video:
//    • Frame 0 → video seconds [0, 1)
//    • Frame 1 → video seconds [1, 2)
//    • Frame N → video seconds [N, N+1)
//
//  For output video frame n (0-indexed):
//    tele_t  = (n / fps) + sync_offset_s
//    tele_a  = floor(tele_t).clamp(0, n_tele - 2)
//    tele_b  = tele_a + 1
//    t       = tele_t - tele_a              (lerp factor in [0, 1))
//
//  Total output frames = (n_tele - 1) * fps
//  sync_offset_s shifts which data appears without changing the frame count.
//
//  --sync-offset <s>
//    Positive: HUD data advances — use when the HUD speed LAGS the dashcam readout.
//    Negative: HUD data retreats — use when the HUD speed LEADS the dashcam readout.
//
//  Architecture:
//    ┌─────────────────────────┐  channel  ┌────────────────────────┐
//    │  RAYON thread pool      │ ────────▶  │  WRITER thread          │
//    │  parallel frame render  │           │  sequential ffmpeg/disk  │
//    └─────────────────────────┘           └────────────────────────┘
///////////////////////////////////////////////////////////////////////////////

use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, mpsc};
use std::thread;

use anyhow::{Context, Result};
use image::RgbaImage;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;

use crate::hud::elements::HudElements;
use crate::hud::interpolation::interpolate;
use crate::hud::renderer::{draw_hud, MinimapCache};
use crate::telemetry::vantrue_frames::TelemetryFrame;

// ─── Public config ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct RenderConfig {
    pub width:           u32,
    pub height:          u32,
    pub fps:             u32,
    /// Unused when GPS is available; kept for API compat and fallback.
    pub max_speed:       f64,
    pub output:          OutputMode,
    pub elements:        HudElements,
    /// Shift the HUD data relative to the video in seconds.
    /// Positive = HUD data moves forward (fixes HUD lagging behind footage).
    /// Negative = HUD data moves backward (fixes HUD running ahead of footage).
    pub sync_offset_s:   f64,
}

#[derive(Debug, Clone)]
pub enum OutputMode {
    /// Write individual PNG frames to a directory
    Frames(PathBuf),
    /// Pipe raw RGBA into ffmpeg → single video file (use .mov/.webm/.mkv for alpha)
    FfmpegPipe(PathBuf),
    /// Both PNG frames AND a video file
    Both { frames_dir: PathBuf, video: PathBuf },
    /// Compose HUD directly over source footage in one ffmpeg pass — no black bg
    ComposeWithSource { source: PathBuf, output: PathBuf },
}

impl Default for RenderConfig {
    fn default() -> Self {
        Self {
            width:           3840,
            height:          2160,
            fps:             30,
            max_speed:       120.0,
            output:          OutputMode::Frames(PathBuf::from("frames")),
            elements:        HudElements::default(),
            sync_offset_s:   0.0,
        }
    }
}

// ─── Job descriptor ───────────────────────────────────────────────────────────

/// One rendered output frame.
#[derive(Debug, Clone)]
pub struct Job {
    /// Output frame index (0, 1, 2, …).
    pub global:     usize,
    /// Index of the earlier telemetry sample.
    pub tele_a:     usize,
    /// Index of the later telemetry sample.
    pub tele_b:     usize,
    /// Lerp factor in [0, 1] between tele_a and tele_b.
    pub t:          f64,
    /// Which g_history entry to use (always == tele_a).
    pub g_hist_idx: usize,
}

// ─── Job builder — the core sync logic ───────────────────────────────────────

/// Build the ordered list of render jobs.
///
/// Rule: 1 TelemetryFrame = 1 second of video.
///
/// For output frame `n` at fps `fps`:
///   tele_t = n / fps + sync_offset_s       (position in telemetry timeline)
///   tele_a = floor(tele_t).clamp(0, n-2)
///   tele_b = tele_a + 1
///   t      = tele_t - tele_a               (sub-second lerp)
///
/// Total output frames = (n_tele - 1) * fps.
/// This is the same regardless of sync_offset_s — the offset only shifts
/// *which* telemetry data is shown at each video second.
pub fn build_jobs(n_tele: usize, fps: u32, sync_offset_s: f64) -> Vec<Job> {
    assert!(n_tele >= 2, "need at least 2 telemetry frames");
    assert!(fps > 0,     "fps must be > 0");

    let fps_f    = fps as f64;
    let total    = (n_tele - 1) * fps as usize;
    let max_a    = n_tele - 2;

    (0..total).map(|n| {
        // Position in the telemetry timeline (seconds) for this output frame
        let tele_t = n as f64 / fps_f + sync_offset_s;

        // Clamp so we never index outside the telemetry array
        let tele_t = tele_t.clamp(0.0, (n_tele - 1) as f64);

        let a = (tele_t.floor() as usize).min(max_a);
        let b = a + 1;
        let t = (tele_t - a as f64).clamp(0.0, 1.0);

        Job { global: n, tele_a: a, tele_b: b, t, g_hist_idx: a }
    }).collect()
}

// ─── Entry point ──────────────────────────────────────────────────────────────

pub fn render(frames: &[TelemetryFrame], cfg: &RenderConfig) -> Result<()> {
    if frames.len() < 2 {
        anyhow::bail!("Need at least 2 telemetry frames to render.");
    }

    // ── Pre-compute caches (shared read-only across rayon threads) ────────────
    let minimap_cache = Arc::new(MinimapCache::build(frames, 280, 30));

    let g_histories: Vec<Vec<(f32, f32)>> = (0..frames.len())
        .map(|i| {
            frames[..=i]
                .iter()
                .rev()
                .take(40)
                .filter_map(|f| f.accel.as_ref().map(|a| (a.x, a.y)))
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect()
        })
        .collect();
    let g_histories = Arc::new(g_histories);

    // ── Build jobs ────────────────────────────────────────────────────────────
    let jobs  = build_jobs(frames.len(), cfg.fps, cfg.sync_offset_s);
    let total = jobs.len();

    println!(
        "  Sync  : {} output frames ({:.1}s @ {}fps)  |  1 tele frame = 1s  |  offset={:+.2}s",
        total,
        total as f64 / cfg.fps as f64,
        cfg.fps,
        cfg.sync_offset_s,
    );

    // ── Progress bar ───────────────────────────────────────────────────────────
    let pb = ProgressBar::new(total as u64);
    pb.set_style(
        ProgressStyle::with_template(
            "[{elapsed_precise}] [{bar:50.cyan/blue}] {pos}/{len} frames  ({per_sec}, ETA {eta})",
        )
            .unwrap()
            .progress_chars("█▉▊▋▌▍▎▏ "),
    );

    match &cfg.output {
        OutputMode::Frames(dir) => {
            std::fs::create_dir_all(dir)?;
            render_frames(frames, &jobs, cfg, dir, &minimap_cache, &g_histories, &pb)
        }
        OutputMode::FfmpegPipe(video) => {
            render_pipe(frames, &jobs, cfg, video, None, &minimap_cache, &g_histories, &pb)
        }
        OutputMode::Both { frames_dir, video } => {
            std::fs::create_dir_all(frames_dir)?;
            render_pipe(frames, &jobs, cfg, video, Some(frames_dir.clone()), &minimap_cache, &g_histories, &pb)
        }
        OutputMode::ComposeWithSource { source, output } => {
            render_compose(frames, &jobs, cfg, source, output, &minimap_cache, &g_histories, &pb)
        }
    }?;

    pb.finish_with_message("Done!");
    Ok(())
}

// ─── PNG-frames path ─────────────────────────────────────────────────────────

fn render_frames(
    telemetry:   &[TelemetryFrame],
    jobs:        &[Job],
    cfg:         &RenderConfig,
    dir:         &PathBuf,
    mm_cache:    &Arc<MinimapCache>,
    g_histories: &Arc<Vec<Vec<(f32, f32)>>>,
    pb:          &ProgressBar,
) -> Result<()> {
    let tele = Arc::new(telemetry.to_vec());
    let cfg  = Arc::new(cfg.clone());
    let pb   = Arc::new(pb.clone());
    let dir  = Arc::new(dir.clone());

    jobs.par_iter().try_for_each(|job| -> Result<()> {
        let img  = render_one(&tele, job, &cfg, mm_cache, g_histories)?;
        let path = dir.join(format!("frame_{:05}.png", job.global));
        img.save(&path).with_context(|| format!("Failed to save {}", path.display()))?;
        pb.inc(1);
        Ok(())
    })
}

// ─── ffmpeg-pipe path ────────────────────────────────────────────────────────

fn render_pipe(
    telemetry:   &[TelemetryFrame],
    jobs:        &[Job],
    cfg:         &RenderConfig,
    video:       &PathBuf,
    frames_dir:  Option<PathBuf>,
    mm_cache:    &Arc<MinimapCache>,
    g_histories: &Arc<Vec<Vec<(f32, f32)>>>,
    pb:          &ProgressBar,
) -> Result<()> {
    let ncpus = num_cpus();
    let (tx, rx) = mpsc::sync_channel::<(usize, RgbaImage)>(ncpus * 3);

    let tele     = Arc::new(telemetry.to_vec());
    let cfg_arc  = Arc::new(cfg.clone());
    let pb_arc   = Arc::new(pb.clone());
    let jobs_arc = Arc::new(jobs.to_vec());

    let mut child  = spawn_ffmpeg(cfg, video)?;
    let stdin_pipe = child.stdin.take().context("ffmpeg stdin unavailable")?;
    let frames_dir_c = frames_dir.clone();

    let writer = thread::spawn(move || -> Result<()> {
        let mut stdin   = stdin_pipe;
        let mut pending = std::collections::BTreeMap::<usize, RgbaImage>::new();
        let mut next    = 0usize;
        for (idx, img) in rx {
            pending.insert(idx, img);
            while let Some(img) = pending.remove(&next) {
                stdin.write_all(img.as_raw())?;
                if let Some(ref d) = frames_dir_c {
                    img.save(d.join(format!("frame_{:05}.png", next)))?;
                }
                next += 1;
            }
        }
        for (_, img) in pending { stdin.write_all(img.as_raw())?; }
        Ok(())
    });

    jobs_arc.par_iter().try_for_each(|job| -> Result<()> {
        let img = render_one(&tele, job, &cfg_arc, mm_cache, g_histories)?;
        tx.send((job.global, img)).context("Writer thread died")?;
        pb_arc.inc(1);
        Ok(())
    })?;
    drop(tx);
    writer.join().map_err(|_| anyhow::anyhow!("Writer thread panicked"))??;
    child.wait().context("ffmpeg process failed")?;
    Ok(())
}

// ─── Compose path ────────────────────────────────────────────────────────────

fn render_compose(
    telemetry:   &[TelemetryFrame],
    jobs:        &[Job],
    cfg:         &RenderConfig,
    source:      &PathBuf,
    output:      &PathBuf,
    mm_cache:    &Arc<MinimapCache>,
    g_histories: &Arc<Vec<Vec<(f32, f32)>>>,
    pb:          &ProgressBar,
) -> Result<()> {
    let ncpus = num_cpus();
    let (tx, rx) = mpsc::sync_channel::<(usize, RgbaImage)>(ncpus * 3);

    let tele     = Arc::new(telemetry.to_vec());
    let cfg_arc  = Arc::new(cfg.clone());
    let pb_arc   = Arc::new(pb.clone());
    let jobs_arc = Arc::new(jobs.to_vec());

    let mut child  = spawn_ffmpeg_compose(cfg, source, output)?;
    let stdin_pipe = child.stdin.take().context("ffmpeg stdin unavailable")?;

    let writer = thread::spawn(move || -> Result<()> {
        let mut stdin   = stdin_pipe;
        let mut pending = std::collections::BTreeMap::<usize, RgbaImage>::new();
        let mut next    = 0usize;
        for (idx, img) in rx {
            pending.insert(idx, img);
            while let Some(img) = pending.remove(&next) {
                stdin.write_all(img.as_raw())?;
                next += 1;
            }
        }
        for (_, img) in pending { stdin.write_all(img.as_raw())?; }
        Ok(())
    });

    jobs_arc.par_iter().try_for_each(|job| -> Result<()> {
        let img = render_one(&tele, job, &cfg_arc, mm_cache, g_histories)?;
        tx.send((job.global, img)).context("Writer thread died")?;
        pb_arc.inc(1);
        Ok(())
    })?;
    drop(tx);
    writer.join().map_err(|_| anyhow::anyhow!("Writer thread panicked"))??;
    child.wait().context("ffmpeg compose process failed")?;
    Ok(())
}

// ─── Core per-frame render ────────────────────────────────────────────────────

fn render_one(
    frames:      &[TelemetryFrame],
    job:         &Job,
    cfg:         &RenderConfig,
    mm_cache:    &MinimapCache,
    g_histories: &[Vec<(f32, f32)>],
) -> Result<RgbaImage> {
    let interp     = interpolate(&frames[job.tele_a], &frames[job.tele_b], job.t);
    let all_frames = &frames[..=(job.tele_a + 1).min(frames.len() - 1)];
    let g_history  = &g_histories[job.g_hist_idx];

    let mut img = RgbaImage::new(cfg.width, cfg.height);
    draw_hud(&mut img, &interp, all_frames, cfg.max_speed, g_history, mm_cache, cfg.elements);
    Ok(img)
}

// ─── ffmpeg helpers ───────────────────────────────────────────────────────────

fn spawn_ffmpeg(cfg: &RenderConfig, out_path: &PathBuf) -> Result<Child> {
    let ext = out_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("mp4")
        .to_lowercase();

    if ext == "mp4" {
        eprintln!(
            "\n⚠️  WARNING: .mp4 has no alpha channel — HUD background will be solid black.\n\
             Use .mov (ProRes), .webm (VP9), or .mkv (FFV1) for transparency,\n\
             or use the `compose` subcommand to burn the HUD directly onto footage.\n"
        );
    }

    let (vcodec, pix_fmt, extra): (&str, &str, Vec<String>) = match ext.as_str() {
        "mov"  => ("prores_ks", "yuva444p10le", vec!["-profile:v".into(), "4444".into()]),
        "webm" => ("libvpx-vp9", "yuva420p",   vec!["-b:v".into(), "0".into(), "-crf".into(), "30".into()]),
        "mkv"  => ("ffv1",       "rgba",        vec![]),
        _      => ("libx264",    "yuv420p",     vec!["-crf".into(), "18".into(), "-preset".into(), "fast".into()]),
    };

    let size_str = format!("{}x{}", cfg.width, cfg.height);
    let fps_str  = cfg.fps.to_string();
    let out_str  = out_path.to_str().context("Invalid output path")?;

    let mut args: Vec<String> = vec![
        "-y".into(),
        "-f".into(), "rawvideo".into(),
        "-pixel_format".into(), "rgba".into(),
        "-video_size".into(), size_str,
        "-framerate".into(), fps_str,
        "-i".into(), "pipe:0".into(),
        "-c:v".into(), vcodec.into(),
        "-pix_fmt".into(), pix_fmt.into(),
    ];
    args.extend(extra);
    args.push(out_str.into());

    Command::new("ffmpeg")
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .context("Failed to spawn ffmpeg — is it installed and on PATH?")
}

pub fn spawn_ffmpeg_compose(cfg: &RenderConfig, source: &PathBuf, out_path: &PathBuf) -> Result<Child> {
    let size_str = format!("{}x{}", cfg.width, cfg.height);
    let fps_str  = cfg.fps.to_string();

    Command::new("ffmpeg")
        .args([
            "-y",
            "-i", source.to_str().context("Invalid source path")?,
            "-f", "rawvideo",
            "-pixel_format", "rgba",
            "-video_size", &size_str,
            "-framerate", &fps_str,
            "-i", "pipe:0",
            "-filter_complex", "[0:v][1:v]overlay=0:0:format=auto",
            "-c:a", "copy",
            "-c:v", "libx264",
            "-pix_fmt", "yuv420p",
            "-crf", "18",
            "-preset", "fast",
            out_path.to_str().context("Invalid output path")?,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::inherit())
        .spawn()
        .context("Failed to spawn ffmpeg for compose")
}

fn num_cpus() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4)
}

// ═══════════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    // ── Frame count ───────────────────────────────────────────────────────────

    #[test]
    fn frame_count_is_n_tele_minus_one_times_fps() {
        assert_eq!(build_jobs(5, 30, 0.0).len(), 4 * 30);
        assert_eq!(build_jobs(2, 30, 0.0).len(), 1 * 30);
        assert_eq!(build_jobs(10, 60, 0.0).len(), 9 * 60);
    }

    #[test]
    fn sync_offset_does_not_change_frame_count() {
        let base    = build_jobs(5, 30, 0.0).len();
        let pos_off = build_jobs(5, 30, 2.5).len();
        let neg_off = build_jobs(5, 30, -1.0).len();
        assert_eq!(base, pos_off);
        assert_eq!(base, neg_off);
    }

    // ── First frame ───────────────────────────────────────────────────────────

    #[test]
    fn first_frame_is_at_tele_zero_with_t_zero() {
        let jobs = build_jobs(5, 30, 0.0);
        let j = &jobs[0];
        assert_eq!(j.global, 0);
        assert_eq!(j.tele_a, 0);
        assert_eq!(j.tele_b, 1);
        assert!((j.t - 0.0).abs() < 1e-12, "expected t=0, got {}", j.t);
    }

    // ── Second boundaries ─────────────────────────────────────────────────────

    #[test]
    fn frame_at_each_second_boundary_has_t_zero_and_correct_tele_a() {
        let fps  = 30u32;
        let jobs = build_jobs(5, fps, 0.0);
        for k in 1..4usize {
            let j = &jobs[k * fps as usize];
            assert_eq!(j.tele_a, k, "at second {k} tele_a should be {k}");
            assert_eq!(j.tele_b, k + 1);
            assert!(j.t < 1e-12, "t at boundary {k} should be 0, got {}", j.t);
        }
    }

    #[test]
    fn frame_at_half_second_has_t_point_five() {
        let fps  = 30u32;
        let jobs = build_jobs(4, fps, 0.0);
        let j    = &jobs[15]; // 15/30 = 0.5s
        assert_eq!(j.tele_a, 0);
        assert!((j.t - 0.5).abs() < 1e-9, "expected t=0.5, got {}", j.t);
    }

    // ── Bounds safety ─────────────────────────────────────────────────────────

    #[test]
    fn all_tele_indices_are_in_bounds() {
        let n    = 7usize;
        let jobs = build_jobs(n, 30, 0.0);
        for j in &jobs {
            assert!(j.tele_a < n - 1, "tele_a={} out of bounds (n={})", j.tele_a, n);
            assert!(j.tele_b < n,     "tele_b={} out of bounds (n={})", j.tele_b, n);
            assert!(j.t >= 0.0 && j.t <= 1.0, "t={} out of [0,1]", j.t);
        }
    }

    #[test]
    fn large_positive_offset_clamps_to_last_pair() {
        let n    = 3usize;
        let jobs = build_jobs(n, 10, 999.0);
        for j in &jobs {
            assert_eq!(j.tele_a, n - 2);
            assert_eq!(j.tele_b, n - 1);
        }
    }

    #[test]
    fn large_negative_offset_clamps_to_first_pair() {
        let jobs = build_jobs(5, 10, -999.0);
        for j in &jobs {
            assert_eq!(j.tele_a, 0);
            assert_eq!(j.tele_b, 1);
        }
    }

    // ── sync_offset correctness ───────────────────────────────────────────────

    #[test]
    fn positive_offset_advances_tele_data() {
        // +0.5s offset at 10fps: frame 0 should be at tele_t=0.5 (halfway into first second)
        let jobs = build_jobs(5, 10, 0.5);
        let j    = &jobs[0];
        assert_eq!(j.tele_a, 0);
        assert!((j.t - 0.5).abs() < 1e-9,
                "frame 0 with +0.5s offset should have t=0.5, got {}", j.t);
    }

    #[test]
    fn positive_offset_one_second_shifts_entire_tele_a_sequence() {
        // +1.0s offset: frame 0 should show tele pair [1, 2] at t=0
        let jobs = build_jobs(5, 10, 1.0);
        let j    = &jobs[0];
        assert_eq!(j.tele_a, 1, "with +1s offset frame 0 should start at tele_a=1");
        assert!(j.t < 1e-9, "t should be 0 at tele second boundary");
    }

    #[test]
    fn negative_offset_clamps_frame_zero_to_start() {
        let jobs = build_jobs(5, 10, -2.0);
        let j    = &jobs[0];
        assert_eq!(j.tele_a, 0);
        assert!(j.t < 1e-9, "negative offset clamps to tele start, t should be 0");
    }

    // ── Monotonicity ─────────────────────────────────────────────────────────

    #[test]
    fn global_index_is_sequential_from_zero() {
        let jobs = build_jobs(5, 30, 0.0);
        for (i, j) in jobs.iter().enumerate() {
            assert_eq!(j.global, i);
        }
    }

    #[test]
    fn t_is_non_decreasing_within_each_tele_second() {
        let jobs = build_jobs(5, 30, 0.0);
        for k in 0..4usize {
            let sec: Vec<_> = jobs.iter().filter(|j| j.tele_a == k).collect();
            for w in sec.windows(2) {
                assert!(w[1].t >= w[0].t,
                        "t should be non-decreasing within tele second {k}");
            }
        }
    }

    #[test]
    fn tele_a_is_non_decreasing_across_all_frames() {
        let jobs = build_jobs(6, 30, 0.0);
        for w in jobs.windows(2) {
            assert!(w[1].tele_a >= w[0].tele_a,
                    "tele_a must not decrease: {} -> {}", w[0].tele_a, w[1].tele_a);
        }
    }

    // ── g_hist_idx ────────────────────────────────────────────────────────────

    #[test]
    fn g_hist_idx_always_equals_tele_a() {
        let jobs = build_jobs(8, 30, 0.0);
        for j in &jobs {
            assert_eq!(j.g_hist_idx, j.tele_a);
        }
    }

    // ── Two-frame edge case ───────────────────────────────────────────────────

    #[test]
    fn two_tele_frames_produces_exactly_fps_jobs() {
        let fps  = 25u32;
        let jobs = build_jobs(2, fps, 0.0);
        assert_eq!(jobs.len(), fps as usize);
        // All jobs must stay within the single pair [0, 1]
        for j in &jobs {
            assert_eq!(j.tele_a, 0);
            assert_eq!(j.tele_b, 1);
        }
    }

    #[test]
    fn two_tele_frames_last_t_is_fps_minus_one_over_fps() {
        let fps  = 10u32;
        let jobs = build_jobs(2, fps, 0.0);
        let last = jobs.last().unwrap();
        let expected = 9.0 / 10.0;
        assert!((last.t - expected).abs() < 1e-9,
                "last frame t={} expected {}", last.t, expected);
    }
}