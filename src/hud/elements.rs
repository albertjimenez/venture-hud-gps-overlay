/// Which HUD widgets to draw.  Each field is `true` = draw it.
#[derive(Debug, Clone, Copy)]
pub struct HudElements {
    pub speedometer:  bool,
    pub gforce:       bool,
    pub compass:      bool,
    pub gps_tag:      bool,
    pub minimap:      bool,
}

impl HudElements {
    /// Everything on — default race/action view
    pub fn full() -> Self {
        Self { speedometer: true, gforce: true, compass: true, gps_tag: true, minimap: true }
    }

    /// Speed + map only — clean driving view
    pub fn default_view() -> Self {
        Self { speedometer: true, gforce: false, compass: true, gps_tag: false, minimap: true }
    }

    /// Speed only — absolute minimum
    pub fn minimal() -> Self {
        Self { speedometer: true, gforce: false, compass: false, gps_tag: false, minimap: false }
    }

    /// Nothing — useful for testing the pipeline
    pub fn none() -> Self {
        Self { speedometer: false, gforce: false, compass: false, gps_tag: false, minimap: false }
    }
}

impl Default for HudElements {
    fn default() -> Self { Self::default_view() }
}
