use serde::{Deserialize, Serialize};

/// One GPS sample from a single ExifTool Doc block.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GpsFrame {
    /// Raw timestamp string as emitted by ExifTool, e.g. "2026:03:07 17:14:45Z"
    pub timestamp: String,

    /// Wall-clock time of this sample as seconds since Unix epoch.
    /// Parsed from GPSDateTime. Used by tests and the inspect command;
    /// the pipeline uses the Doc index (1 frame = 1 second) for sync.
    pub epoch_s: f64,

    /// Seconds since the first telemetry sample (epoch_s − epoch_s[0]).
    /// Always 0.0 for Doc1. Useful for debugging sync issues via `inspect`.
    pub time_s: f64,

    pub latitude:  f64,
    pub longitude: f64,

    /// Speed in km/h.
    /// Converted from the unit given by GPSSpeedRef:
    ///   "K" = km/h (pass-through)
    ///   "M" = mph  → ×1.60934
    ///   "N" = knots → ×1.852   ← Vantrue default when ref is absent
    pub speed: Option<f64>,

    /// Heading in degrees (0–360, 0 = North).
    pub heading: Option<f64>,
}

/// One accelerometer sample from a single ExifTool Doc block.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AccelerometerFrame {
    /// Lateral G (left/right)
    pub x: f32,
    /// Longitudinal G (forward/brake)
    pub y: f32,
    /// Vertical G
    pub z: f32,
}

/// One second of dashcam telemetry (one ExifTool Doc block).
///
/// **Sync contract:** 1 TelemetryFrame = exactly 1 second of video.
///   Doc1  → video seconds [0, 1)
///   Doc2  → video seconds [1, 2)
///   DocN  → video seconds [N-1, N)
///
/// The pipeline never uses GPS timestamps for timing — it uses the Doc
/// index directly. Use `--sync-offset` on the CLI to nudge alignment.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TelemetryFrame {
    /// ExifTool document group identifier, e.g. "Doc1", "Doc42".
    pub doc_id: String,
    pub gps:    Option<GpsFrame>,
    pub accel:  Option<AccelerometerFrame>,
}