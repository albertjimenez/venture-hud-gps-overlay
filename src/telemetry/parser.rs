use serde_json::Value;
use std::collections::{BTreeMap, HashMap};

use crate::telemetry::vantrue_frames::{AccelerometerFrame, GpsFrame, TelemetryFrame};

// ─── GPS coordinate parsing ───────────────────────────────────────────────────

/// Convert a DMS string like `"39 deg 59' 2.18\" N"` to decimal degrees.
fn dms_to_decimal(dms: &str) -> Option<f64> {
    let dms = dms.trim();
    let parts: Vec<&str> = dms.split(" deg ").collect();
    if parts.len() != 2 { return None; }

    let deg: f64 = parts[0].parse().ok()?;
    let rest = parts[1];
    let min_parts: Vec<&str> = rest.split('\'').collect();
    if min_parts.len() != 2 { return None; }

    let min: f64 = min_parts[0].trim().parse().ok()?;
    let sec_dir = min_parts[1].trim();
    let sec_parts: Vec<&str> = sec_dir.split_whitespace().collect();
    if sec_parts.len() != 2 { return None; }

    let sec: f64 = sec_parts[0].trim_end_matches('"').parse().ok()?;
    let dir = sec_parts[1];
    let sign = if dir.starts_with('S') || dir.starts_with('W') { -1.0 } else { 1.0 };

    Some(sign * (deg + min / 60.0 + sec / 3600.0))
}

// ─── Timestamp parsing ────────────────────────────────────────────────────────

/// Parse an ExifTool datetime string into seconds since the Unix epoch.
///
/// Handles the formats ExifTool emits:
///   "2026:03:07 17:14:45Z"
///   "2026:03:07 17:14:45+00:00"
///   "2026:03:07 17:14:45"
///   "2026:03:07 17:14:45.123Z"   (fractional seconds)
///
/// All inputs are treated as UTC. Returns `None` on any parse failure.
pub fn parse_datetime_to_epoch(s: &str) -> Option<f64> {
    let s = s.trim();
    // Strip timezone: everything from '+' or trailing 'Z'
    let s = if let Some(idx) = s.find('+') { &s[..idx] }
    else if s.ends_with('Z')        { &s[..s.len() - 1] }
    else                            { s };
    let s = s.trim();

    let parts: Vec<&str> = s.splitn(2, ' ').collect();
    if parts.len() != 2 { return None; }

    let date_parts: Vec<u64> = parts[0].split(':').filter_map(|p| p.parse().ok()).collect();
    let time_parts: Vec<f64> = parts[1].split(':').filter_map(|p| p.parse().ok()).collect();

    if date_parts.len() < 3 || time_parts.len() < 3 { return None; }

    let (year, month, day) = (date_parts[0], date_parts[1], date_parts[2]);
    let (hour, minute, second) = (time_parts[0] as u64, time_parts[1] as u64, time_parts[2]);

    let days  = days_since_epoch(year, month, day)?;
    let epoch = days as f64 * 86_400.0
        + hour   as f64 * 3_600.0
        + minute as f64 * 60.0
        + second;
    Some(epoch)
}

fn days_since_epoch(year: u64, month: u64, day: u64) -> Option<i64> {
    if year < 1970 || month < 1 || month > 12 || day < 1 { return None; }

    let y = year  as i64;
    let m = month as i64;
    let d = day   as i64;

    let leap = (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0);
    let month_days: [i64; 12] = [
        31, if leap { 29 } else { 28 }, 31, 30, 31, 30,
        31, 31, 30, 31, 30, 31,
    ];

    let mut days = (y - 1970) * 365 + leap_days_since_1970(y);
    for mi in 0..(m - 1) { days += month_days[mi as usize]; }
    days += d - 1;
    Some(days)
}

fn leap_days_since_1970(year: i64) -> i64 {
    let y = year - 1;
    (y / 4 - 1970 / 4) - (y / 100 - 1970 / 100) + (y / 400 - 1970 / 400)
}

// ─── Accelerometer parsing ────────────────────────────────────────────────────

/// Parse a Vantrue accelerometer string like `"-0.02 -0.776 -0.342"` → x, y, z.
fn parse_accel(s: &str) -> Option<AccelerometerFrame> {
    let vals: Vec<f32> = s.split_whitespace()
        .filter_map(|v| v.parse().ok())
        .collect();
    if vals.len() != 3 { return None; }
    Some(AccelerometerFrame { x: vals[0], y: vals[1], z: vals[2] })
}

// ─── Speed conversion ─────────────────────────────────────────────────────────

/// Convert GPS speed to km/h using the GPSSpeedRef unit code.
///   "K" = km/h  → pass through
///   "M" = mph   → ×1.60934
///   "N" = knots → ×1.852
///   absent / unknown → pass through as km/h (Vantrue native unit)
///
/// Vantrue stores GPS speed natively in km/h. Even with -u/-a exiftool flags,
/// GPSSpeedRef may be absent from individual Doc blocks. Defaulting the
/// unknown case to km/h (passthrough) keeps Vantrue correct. Only an
/// explicit "N" tag triggers knot conversion.
fn to_kmh(speed: f64, speed_ref: &str) -> f64 {
    match speed_ref.trim().to_uppercase().as_str() {
        "K" => speed,
        "M" => speed * 1.609_34,
        "N" => speed * 1.852,
        _   => speed,  // absent / unknown → already km/h
    }
}

// ─── Main telemetry parser ────────────────────────────────────────────────────

/// Parse exiftool JSON output into a sorted `Vec<TelemetryFrame>`.
///
/// Each frame corresponds to one ExifTool Doc block, which covers exactly
/// one second of dashcam video. Frames are sorted by Doc number so that
/// `frames[0]` = second 0, `frames[1]` = second 1, etc.
///
/// Fields populated:
///   - `gps.epoch_s`  — wall-clock seconds since Unix epoch from GPSDateTime
///   - `gps.time_s`   — seconds relative to first GPS sample (0.0 at Doc1)
///   - `gps.speed`    — always in km/h (converted via GPSSpeedRef)
pub fn parse_telemetry(json: &Value) -> Vec<TelemetryFrame> {
    let obj = match json.as_object() {
        Some(o) => o,
        None    => return vec![],
    };

    // ── Group all "Doc*:Field" entries by doc_id ────────────────────────────
    let mut docs: BTreeMap<String, HashMap<String, Value>> = BTreeMap::new();
    for (key, value) in obj.iter() {
        if let Some(colon) = key.find(':') {
            let doc_id = &key[..colon];
            if doc_id.starts_with("Doc") {
                docs.entry(doc_id.to_string())
                    .or_default()
                    .insert(key[colon + 1..].to_string(), value.clone());
            }
        }
    }

    // ── Parse each Doc block ────────────────────────────────────────────────
    let mut frames: Vec<TelemetryFrame> = docs.into_iter().map(|(doc_id, fields)| {
        // Speed unit for this doc — fall back to top-level tag, then assume knots
        let speed_ref = fields
            .get("GPSSpeedRef")
            .and_then(|v| v.as_str())
            .or_else(|| obj.get("GPSSpeedRef").and_then(|v| v.as_str()))
            .unwrap_or("K")  // absent → km/h passthrough (Vantrue native unit)
            .to_string();

        let gps = if let (Some(lat_str), Some(lon_str), Some(ts)) = (
            fields.get("GPSLatitude").and_then(|v| v.as_str()),
            fields.get("GPSLongitude").and_then(|v| v.as_str()),
            fields.get("GPSDateTime").and_then(|v| v.as_str()),
        ) {
            let lat     = dms_to_decimal(lat_str).unwrap_or(0.0);
            let lon     = dms_to_decimal(lon_str).unwrap_or(0.0);
            let epoch_s = parse_datetime_to_epoch(ts).unwrap_or(0.0);
            let speed   = fields.get("GPSSpeed")
                .and_then(|v| v.as_f64())
                .map(|s| to_kmh(s, &speed_ref));
            let heading = fields.get("GPSTrack").and_then(|v| v.as_f64());

            Some(GpsFrame {
                timestamp: ts.to_string(),
                epoch_s,
                time_s: 0.0,  // filled in the post-pass below
                latitude: lat,
                longitude: lon,
                speed,
                heading,
            })
        } else {
            None
        };

        let accel = fields
            .get("Accelerometer")
            .and_then(|v| v.as_str())
            .and_then(parse_accel);

        TelemetryFrame { doc_id, gps, accel }
    }).collect();

    // ── Sort by Doc number ──────────────────────────────────────────────────
    frames.sort_by_key(|f| {
        f.doc_id.trim_start_matches("Doc").parse::<u32>().unwrap_or(0)
    });

    // ── Post-pass: compute time_s relative to first GPS epoch ───────────────
    let first_epoch = frames.iter()
        .filter_map(|f| f.gps.as_ref())
        .find(|g| g.epoch_s > 0.0)
        .map(|g| g.epoch_s)
        .unwrap_or(0.0);

    for frame in frames.iter_mut() {
        if let Some(gps) = frame.gps.as_mut() {
            gps.time_s = if gps.epoch_s > 0.0 { gps.epoch_s - first_epoch } else { 0.0 };
        }
    }

    frames
}

// ═══════════════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── dms_to_decimal ────────────────────────────────────────────────────────

    #[test]
    fn dms_north_is_positive() {
        let v = dms_to_decimal("39 deg 59' 2.18\" N").unwrap();
        let expected = 39.0 + 59.0 / 60.0 + 2.18 / 3600.0;
        assert!((v - expected).abs() < 1e-9, "got {v}");
    }

    #[test]
    fn dms_south_is_negative() {
        let v = dms_to_decimal("33 deg 52' 30.00\" S").unwrap();
        let expected = -(33.0 + 52.0 / 60.0 + 30.0 / 3600.0);
        assert!((v - expected).abs() < 1e-9, "got {v}");
    }

    #[test]
    fn dms_west_is_negative() {
        let v = dms_to_decimal("3 deg 55' 15.60\" W").unwrap();
        assert!(v < 0.0, "west should be negative, got {v}");
    }

    #[test]
    fn dms_east_is_positive() {
        let v = dms_to_decimal("3 deg 55' 15.60\" E").unwrap();
        assert!(v > 0.0, "east should be positive, got {v}");
    }

    #[test]
    fn dms_zero_is_zero() {
        let v = dms_to_decimal("0 deg 0' 0.00\" N").unwrap();
        assert!((v - 0.0).abs() < 1e-12);
    }

    #[test]
    fn dms_invalid_returns_none() {
        assert!(dms_to_decimal("not a coordinate").is_none());
        assert!(dms_to_decimal("").is_none());
        assert!(dms_to_decimal("123.456").is_none());
    }

    // ── parse_datetime_to_epoch ───────────────────────────────────────────────

    #[test]
    fn unix_epoch_parses_to_zero() {
        let e = parse_datetime_to_epoch("1970:01:01 00:00:00Z").unwrap();
        assert!((e - 0.0).abs() < 1e-9, "Unix epoch should be 0.0, got {e}");
    }

    #[test]
    fn known_date_is_in_plausible_range() {
        let e = parse_datetime_to_epoch("2026:03:07 17:14:45Z").unwrap();
        assert!(e > 1_700_000_000.0, "epoch too small: {e}");
        assert!(e < 2_000_000_000.0, "epoch too large: {e}");
    }

    #[test]
    fn consecutive_seconds_differ_by_one() {
        let a = parse_datetime_to_epoch("2026:03:07 17:14:45Z").unwrap();
        let b = parse_datetime_to_epoch("2026:03:07 17:14:46Z").unwrap();
        assert!((b - a - 1.0).abs() < 1e-9, "should differ by 1.0s, diff={}", b - a);
    }

    #[test]
    fn midnight_rollover_is_correct() {
        let a = parse_datetime_to_epoch("2026:03:07 23:59:59Z").unwrap();
        let b = parse_datetime_to_epoch("2026:03:08 00:00:00Z").unwrap();
        assert!((b - a - 1.0).abs() < 1e-9, "midnight rollover diff={}", b - a);
    }

    #[test]
    fn no_tz_suffix_parses_ok() {
        assert!(parse_datetime_to_epoch("2026:03:07 17:14:45").is_some());
    }

    #[test]
    fn plus_offset_tz_is_stripped() {
        let a = parse_datetime_to_epoch("2026:03:07 17:14:45Z").unwrap();
        let b = parse_datetime_to_epoch("2026:03:07 17:14:45+00:00").unwrap();
        assert!((a - b).abs() < 1e-9);
    }

    #[test]
    fn fractional_seconds_parse_ok() {
        let a = parse_datetime_to_epoch("2026:03:07 17:14:45Z").unwrap();
        let b = parse_datetime_to_epoch("2026:03:07 17:14:45.500Z").unwrap();
        assert!((b - a - 0.5).abs() < 1e-6, "fractional diff={}", b - a);
    }

    #[test]
    fn invalid_datetime_returns_none() {
        assert!(parse_datetime_to_epoch("not a date").is_none());
        assert!(parse_datetime_to_epoch("").is_none());
        assert!(parse_datetime_to_epoch("2026:13:01 00:00:00Z").is_none()); // month 13
    }

    // ── to_kmh ────────────────────────────────────────────────────────────────

    #[test]
    fn knots_to_kmh() {
        let v = to_kmh(1.0, "N");
        assert!((v - 1.852).abs() < 1e-9);
    }

    #[test]
    fn kmh_passthrough() {
        let v = to_kmh(100.0, "K");
        assert!((v - 100.0).abs() < 1e-9);
    }

    #[test]
    fn mph_to_kmh() {
        let v = to_kmh(60.0, "M");
        assert!((v - 60.0 * 1.609_34).abs() < 1e-3);
    }

    #[test]
    fn unknown_ref_passes_through_as_kmh() {
        // Unknown ref → treat as already km/h (passthrough), not knots
        let v = to_kmh(1.0, "X");
        assert!((v - 1.0).abs() < 1e-9, "unknown ref should pass through, got {v}");
    }

    #[test]
    fn empty_ref_passes_through_as_kmh() {
        // Absent/empty ref → treat as already km/h (Vantrue native unit)
        let v = to_kmh(1.0, "");
        assert!((v - 1.0).abs() < 1e-9, "empty ref should pass through, got {v}");
    }

    // ── parse_accel ───────────────────────────────────────────────────────────

    #[test]
    fn parses_three_signed_floats() {
        let a = parse_accel("-0.021 0.043 -0.982").unwrap();
        assert!((a.x + 0.021).abs() < 1e-6);
        assert!((a.y - 0.043).abs() < 1e-6);
        assert!((a.z + 0.982).abs() < 1e-6);
    }

    #[test]
    fn too_few_components_returns_none() {
        assert!(parse_accel("0.1 0.2").is_none());
    }

    #[test]
    fn too_many_components_returns_none() {
        assert!(parse_accel("0.1 0.2 0.3 0.4").is_none());
    }

    #[test]
    fn empty_string_returns_none() {
        assert!(parse_accel("").is_none());
    }

    // ── parse_telemetry integration ───────────────────────────────────────────

    #[test]
    fn frames_sorted_by_doc_number() {
        let json = json!({
            "Doc3:GPSDateTime":  "2026:01:01 00:00:02Z",
            "Doc3:GPSLatitude":  "1 deg 0' 0.00\" N",
            "Doc3:GPSLongitude": "1 deg 0' 0.00\" E",
            "Doc3:GPSSpeed":     "10", "Doc3:GPSSpeedRef": "K",
            "Doc1:GPSDateTime":  "2026:01:01 00:00:00Z",
            "Doc1:GPSLatitude":  "1 deg 0' 0.00\" N",
            "Doc1:GPSLongitude": "1 deg 0' 0.00\" E",
            "Doc1:GPSSpeed":     "5",  "Doc1:GPSSpeedRef": "K",
            "Doc2:GPSDateTime":  "2026:01:01 00:00:01Z",
            "Doc2:GPSLatitude":  "1 deg 0' 0.00\" N",
            "Doc2:GPSLongitude": "1 deg 0' 0.00\" E",
            "Doc2:GPSSpeed":     "7",  "Doc2:GPSSpeedRef": "K",
        });
        let frames = parse_telemetry(&json);
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0].doc_id, "Doc1");
        assert_eq!(frames[1].doc_id, "Doc2");
        assert_eq!(frames[2].doc_id, "Doc3");
    }

    #[test]
    fn time_s_starts_at_zero_and_increments_by_one() {
        let json = json!({
            "Doc1:GPSDateTime":  "2026:01:01 00:00:00Z",
            "Doc1:GPSLatitude":  "1 deg 0' 0.00\" N",
            "Doc1:GPSLongitude": "1 deg 0' 0.00\" E",
            "Doc2:GPSDateTime":  "2026:01:01 00:00:01Z",
            "Doc2:GPSLatitude":  "1 deg 0' 0.00\" N",
            "Doc2:GPSLongitude": "1 deg 0' 0.00\" E",
            "Doc3:GPSDateTime":  "2026:01:01 00:00:02Z",
            "Doc3:GPSLatitude":  "1 deg 0' 0.00\" N",
            "Doc3:GPSLongitude": "1 deg 0' 0.00\" E",
        });
        let frames = parse_telemetry(&json);
        assert_eq!(frames.len(), 3);
        let t: Vec<f64> = frames.iter()
            .map(|f| f.gps.as_ref().unwrap().time_s)
            .collect();
        assert!((t[0] - 0.0).abs() < 1e-9, "Doc1 time_s={}", t[0]);
        assert!((t[1] - 1.0).abs() < 1e-9, "Doc2 time_s={}", t[1]);
        assert!((t[2] - 2.0).abs() < 1e-9, "Doc3 time_s={}", t[2]);
    }

    #[test]
    fn speed_defaults_to_kmh_passthrough_when_ref_absent() {
        // Vantrue stores speed natively in km/h; absent GPSSpeedRef → passthrough
        let json = json!({
            "Doc1:GPSDateTime":  "2026:01:01 00:00:00Z",
            "Doc1:GPSLatitude":  "1 deg 0' 0.00\" N",
            "Doc1:GPSLongitude": "1 deg 0' 0.00\" E",
            "Doc1:GPSSpeed":     72,
        });
        let frames = parse_telemetry(&json);
        let speed = frames[0].gps.as_ref().unwrap().speed.unwrap();
        assert!((speed - 72.0).abs() < 1e-6,
                "absent ref: 72 should pass through as 72 km/h, got {speed}");
    }

    #[test]
    fn speed_ref_k_passes_through_as_kmh() {
        let json = json!({
            "Doc1:GPSDateTime":  "2026:01:01 00:00:00Z",
            "Doc1:GPSLatitude":  "1 deg 0' 0.00\" N",
            "Doc1:GPSLongitude": "1 deg 0' 0.00\" E",
            "Doc1:GPSSpeed":     72,
            "Doc1:GPSSpeedRef":  "K",
        });
        let frames = parse_telemetry(&json);
        let speed = frames[0].gps.as_ref().unwrap().speed.unwrap();
        assert!((speed - 72.0).abs() < 1e-6, "got {speed}");
    }

    #[test]
    fn accel_only_doc_has_none_gps() {
        let json = json!({
            "Doc1:Accelerometer": "-0.01 0.04 -0.98",
        });
        let frames = parse_telemetry(&json);
        assert_eq!(frames.len(), 1);
        assert!(frames[0].gps.is_none());
        assert!(frames[0].accel.is_some());
    }

    #[test]
    fn empty_object_returns_empty_vec() {
        let frames = parse_telemetry(&json!({}));
        assert!(frames.is_empty());
    }

    #[test]
    fn non_object_json_returns_empty_vec() {
        assert!(parse_telemetry(&json!(null)).is_empty());
        assert!(parse_telemetry(&json!([1, 2, 3])).is_empty());
    }

    #[test]
    fn top_level_speed_ref_used_as_fallback() {
        // GPSSpeedRef at top level (not in Doc), should apply to all docs
        let json = json!({
            "GPSSpeedRef": "K",
            "Doc1:GPSDateTime":  "2026:01:01 00:00:00Z",
            "Doc1:GPSLatitude":  "1 deg 0' 0.00\" N",
            "Doc1:GPSLongitude": "1 deg 0' 0.00\" E",
            "Doc1:GPSSpeed":     55,
        });
        let frames = parse_telemetry(&json);
        let speed = frames[0].gps.as_ref().unwrap().speed.unwrap();
        assert!((speed - 55.0).abs() < 1e-6,
                "top-level K ref should give 55 km/h, got {speed}");
    }
}