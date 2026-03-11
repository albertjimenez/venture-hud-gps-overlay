use crate::telemetry::vantrue_frames::{AccelerometerFrame, GpsFrame, TelemetryFrame};

#[inline]
pub fn lerp(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

fn interp_opt(a: Option<f64>, b: Option<f64>, t: f64) -> Option<f64> {
    match (a, b) {
        (Some(v1), Some(v2)) => Some(lerp(v1, v2, t)),
        (Some(v), None) | (None, Some(v)) => Some(v),
        _ => None,
    }
}

fn interp_accel(
    a: &Option<AccelerometerFrame>,
    b: &Option<AccelerometerFrame>,
    t: f64,
) -> Option<AccelerometerFrame> {
    match (a, b) {
        (Some(ax), Some(bx)) => Some(AccelerometerFrame {
            x: lerp(ax.x as f64, bx.x as f64, t) as f32,
            y: lerp(ax.y as f64, bx.y as f64, t) as f32,
            z: lerp(ax.z as f64, bx.z as f64, t) as f32,
        }),
        (Some(ax), None) | (None, Some(ax)) => Some(ax.clone()),
        _ => None,
    }
}

fn interp_gps(a: &Option<GpsFrame>, b: &Option<GpsFrame>, t: f64) -> Option<GpsFrame> {
    match (a, b) {
        (Some(ga), Some(gb)) => Some(GpsFrame {
            timestamp: ga.timestamp.clone(),
            latitude: lerp(ga.latitude, gb.latitude, t),
            longitude: lerp(ga.longitude, gb.longitude, t),
            speed: interp_opt(ga.speed, gb.speed, t),
            heading: interp_opt(ga.heading, gb.heading, t),
            epoch_s: interp_opt(Option::from(ga.epoch_s), Option::from(gb.epoch_s), t)?,
            time_s: interp_opt(Option::from(ga.time_s), Option::from(gb.time_s), t)?,
        }),
        (Some(g), None) | (None, Some(g)) => Some(g.clone()),
        _ => None,
    }
}

/// Interpolate between two telemetry frames at position t in [0, 1]
pub fn interpolate(a: &TelemetryFrame, b: &TelemetryFrame, t: f64) -> TelemetryFrame {
    TelemetryFrame {
        doc_id: a.doc_id.clone(),
        gps: interp_gps(&a.gps, &b.gps, t),
        accel: interp_accel(&a.accel, &b.accel, t),
    }
}