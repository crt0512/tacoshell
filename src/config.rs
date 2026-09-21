// the baked in config, see schema.rs for whats in it and build.rs for how it gets here

use eframe::egui::Color32;

pub use crate::schema::*;

mod baked {
    include!(concat!(env!("OUT_DIR"), "/baked.rs"));
}

pub fn load() -> Config {
    // build.rs already parsed and checked this exact text, so this cant really fail
    let cfg: Config = toml::from_str(baked::CONFIG).expect("baked config broken, clean and rebuild");
    debug_assert_eq!(cfg.validate(), Ok(()));
    cfg
}

pub fn icon_png() -> Option<&'static [u8]> {
    baked::ICON
}

pub fn baked_toml() -> &'static str {
    baked::CONFIG
}

pub fn meta() -> &'static str {
    baked::META
}

pub fn color(hex: &str) -> Color32 {
    let [r, g, b] = parse_color(hex).unwrap_or([255, 0, 255]);
    Color32::from_rgb(r, g, b)
}

/// "{name}" -> value for every pair
pub fn fill(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = template.to_string();
    for (name, value) in vars {
        out = out.replace(&format!("{{{name}}}"), value);
    }
    out
}
