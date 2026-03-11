use std::error::Error;
use exiftool::ExifTool;
use serde_json::Value;
use std::path::Path;

/// Runs exiftool and returns raw JSON.
///
/// Flags:
///   -ee extract embedded (track-level) metadata from all sub-documents
///   -G3 prefix every key with its document group: "Doc1:GPSSpeed", etc.
///   -s short tag names (no spaces)
///   -a allow duplicate tags (required for multi-Doc GPS files)
///   -u include unknown/vendor tags — this is what surfaces GPSSpeedRef
///         on Vantrue files; without it the tag is silently omitted and the
///         parser falls back to knots, doubling the speed value.
pub fn extract_metadata(file_path: &Path) -> Result<Value, Box<dyn Error>> {
    let exiftool = ExifTool::new()?;
    let args = ["-ee", "-G3", "-s", "-a", "-u"];
    let metadata_json: Value = exiftool.json(file_path, &args)?;
    Ok(metadata_json)
}