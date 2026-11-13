// Horizon's official Minimap, formerly the WebAssembly mod and before that a
// built-in extra: a map of the terrain around the player in a corner, in map
// colours, square or round, north up or turning with the player, drawn by
// Horizon's own minimap. Its settings are the WebAssembly mod's files
// (`horizon-mods/.horizon-mod-data/minimap/corner`, `size`, `shape`, `zoom`,
// `rotate`), so a player keeps them. This file is both the mod's Cargo
// library (its tests) and the module the bundle adds to the game.

/// The mod's id (its settings folder), name and version.
pub const ID: &str = "minimap";
pub const NAME: &str = "Minimap";
pub const VERSION: &str = "2.0.0";
pub const DESCRIPTION: &str = "A map of the terrain around you in a corner, in map colours, square or round, north up or turning with you. Horizon's own.";
/// The corners, as the settings files name them and the page labels them.
pub const CORNERS: [(&str, &str); 4] = [
    ("top_left", "Top left"),
    ("top_right", "Top right"),
    ("bottom_left", "Bottom left"),
    ("bottom_right", "Bottom right"),
];
/// Sides in GUI pixels, and blocks per map pixel.
pub const SIZES: [(u8, &str); 3] = [(64, "Small"), (96, "Medium"), (128, "Large")];
pub const ZOOMS: [f32; 4] = [0.5, 1.0, 2.0, 4.0];

/// The settings, as the WebAssembly Minimap kept them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// Indices in `CORNERS`, `SIZES` and `ZOOMS`.
    pub corner: usize,
    pub size: usize,
    pub round: bool,
    pub zoom: usize,
    pub rotate: bool,
    pub show_coordinates: bool,
    pub outline: usize,
}

impl Settings {
    /// From the stored values; the extra's defaults (top right, medium,
    /// square, 1 block per pixel, north up) for the ones missing.
    pub fn read(get: impl Fn(&str) -> Option<String>) -> Self {
        let get = |key: &str| get(key).map(|v| v.trim().to_owned());
        let find = |key: &str, values: &[String], default: usize| {
            get(key)
                .and_then(|v| values.iter().position(|s| *s == v))
                .unwrap_or(default)
        };
        let corners: Vec<String> = CORNERS.iter().map(|c| c.0.to_owned()).collect();
        let sizes: Vec<String> = SIZES.iter().map(|s| s.0.to_string()).collect();
        let zooms: Vec<String> = ZOOMS.iter().map(f32::to_string).collect();
        Self {
            corner: find("corner", &corners, 1),
            size: find("size", &sizes, 1),
            round: get("shape").as_deref() == Some("round"),
            zoom: find("zoom", &zooms, 1),
            rotate: get("rotate").as_deref() == Some("true"),
            show_coordinates: get("show_coordinates").as_deref() == Some("true"),
            outline: find(
                "outline",
                &["black".into(), "stone".into(), "bamboo".into()],
                0,
            ),
        }
    }
    pub fn labels(&self) -> Vec<String> {
        let n = ZOOMS[self.zoom];
        let blocks = if n < 1.0 {
            "½ block".to_owned()
        } else if n == 1.0 {
            "1 block".to_owned()
        } else {
            format!("{n} blocks")
        };
        vec![
            format!("Corner: {}", CORNERS[self.corner].1),
            format!("Size: {}", SIZES[self.size].1),
            format!("Shape: {}", if self.round { "Round" } else { "Square" }),
            format!("Zoom: {blocks} per pixel"),
            format!(
                "Turn with the player: {}",
                if self.rotate { "ON" } else { "OFF" }
            ),
            format!(
                "Show Coordinates: {}",
                if self.show_coordinates { "ON" } else { "OFF" }
            ),
            format!(
                "Minimap Outline: {}",
                ["Black", "Stone", "Bamboo"][self.outline]
            ),
        ]
    }
    pub fn click(&mut self, i: usize) {
        let next = |i: usize, n: usize| (i + 1) % n;
        if i == 5 {
            self.show_coordinates = !self.show_coordinates;
            return;
        }
        if i == 6 {
            self.outline = next(self.outline, 3);
            return;
        }
        match i {
            0 => self.corner = next(self.corner, CORNERS.len()),
            1 => self.size = next(self.size, SIZES.len()),
            2 => self.round = !self.round,
            3 => self.zoom = next(self.zoom, ZOOMS.len()),
            4 => self.rotate = !self.rotate,
            _ => {}
        }
    }
    pub fn stored(&self) -> [(&'static str, String); 7] {
        [
            ("corner", CORNERS[self.corner].0.to_owned()),
            ("size", SIZES[self.size].0.to_string()),
            (
                "shape",
                if self.round { "round" } else { "square" }.to_owned(),
            ),
            ("zoom", ZOOMS[self.zoom].to_string()),
            (
                "rotate",
                if self.rotate { "true" } else { "false" }.to_owned(),
            ),
            ("show_coordinates", self.show_coordinates.to_string()),
            (
                "outline",
                ["black", "stone", "bamboo"][self.outline].to_owned(),
            ),
        ]
    }
    /// What Horizon's minimap draws: (side, blocks per pixel).
    pub fn view(&self) -> (f32, f32) {
        (f32::from(SIZES[self.size].0), ZOOMS[self.zoom])
    }
}

thread_local! {
    /// The settings, read once from the files, then kept.
    static SETTINGS: std::cell::Cell<Option<Settings>> = const { std::cell::Cell::new(None) };
}

/// The settings, read once through `get` (one file per key).
pub fn settings(get: impl Fn(&str) -> Option<String>) -> Settings {
    SETTINGS.with(|s| {
        let found = s.get().unwrap_or_else(|| Settings::read(get));
        s.set(Some(found));
        found
    })
}

/// Replaces the settings (the settings page changed them).
pub fn set_settings(settings: Settings) {
    SETTINGS.with(|s| s.set(Some(settings)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_writes_the_webassembly_mods_settings() {
        let none = |_: &str| None;
        let s = Settings::read(none);
        assert_eq!(
            (s.corner, s.size, s.round, s.zoom, s.rotate),
            (1, 1, false, 1, false)
        );
        let files = [
            ("corner", "bottom_left"),
            ("size", "128\n"),
            ("shape", "round"),
            ("zoom", "0.5"),
            ("rotate", "true"),
        ];
        let get = |k: &str| files.iter().find(|f| f.0 == k).map(|f| f.1.to_owned());
        let mut s = Settings::read(get);
        assert_eq!(s.view(), (128.0, 0.5));
        assert_eq!(
            s.labels(),
            [
                "Corner: Bottom left",
                "Size: Large",
                "Shape: Round",
                "Zoom: ½ block per pixel",
                "Turn with the player: ON",
                "Show Coordinates: OFF",
                "Minimap Outline: Black"
            ]
        );
        s.click(3);
        s.click(3);
        assert_eq!(s.stored()[3], ("zoom", "2".to_owned()));
        let round = s.stored();
        let back = Settings::read(|k: &str| round.iter().find(|f| f.0 == k).map(|f| f.1.clone()));
        assert_eq!(back, s);
    }

    #[test]
    fn optional_features_keep_the_original_appearance_by_default() {
        let settings = Settings::read(|_| None);
        assert_eq!(settings.view(), (96.0, 1.0));
        assert!(!settings.show_coordinates);
        assert_eq!(settings.outline, 0);
        let invalid = Settings::read(|key| match key {
            "outline" => Some("unknown".into()),
            "show_coordinates" => Some("invalid".into()),
            _ => None,
        });
        assert_eq!(invalid, settings);
    }

    #[test]
    fn coordinates_and_each_outline_toggle_and_round_trip() {
        let mut settings = Settings::read(|_| None);
        for expected in ["stone", "bamboo", "black"] {
            settings.click(5);
            settings.click(6);
            let stored = settings.stored();
            assert_eq!(
                stored.iter().find(|(key, _)| *key == "outline").unwrap().1,
                expected
            );
            let restored = Settings::read(|key| {
                stored
                    .iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, v)| format!("{v}\n"))
            });
            assert_eq!(restored, settings);
        }
        assert!(settings.show_coordinates);
        settings.click(5);
        assert!(!settings.show_coordinates);
    }

    #[test]
    fn every_stored_value_of_earlier_versions_is_read_and_the_buttons_go_round() {
        let read = |pairs: &[(&str, &str)]| {
            Settings::read(|k: &str| {
                pairs
                    .iter()
                    .find(|p| p.0 == k)
                    .map(|p| format!("{}\n", p.1))
            })
        };
        // The values the files hold: each corner, size and zoom, by position.
        for (i, name) in ["top_left", "top_right", "bottom_left", "bottom_right"]
            .into_iter()
            .enumerate()
        {
            assert_eq!(read(&[("corner", name)]).corner, i, "{name}");
        }
        for (i, size) in ["64", "96", "128"].into_iter().enumerate() {
            assert_eq!(read(&[("size", size)]).size, i, "{size}");
        }
        for (i, zoom) in ["0.5", "1", "2", "4"].into_iter().enumerate() {
            assert_eq!(read(&[("zoom", zoom)]).zoom, i, "{zoom}");
        }
        // Only "round" is round and only "true" turns the map; anything else is
        // the default, as is a corner, size or zoom the mod does not have.
        assert!(!read(&[("shape", "square")]).round);
        assert!(!read(&[("rotate", "yes")]).rotate);
        let unknown = read(&[("corner", "middle"), ("size", "100"), ("zoom", "3")]);
        assert_eq!((unknown.corner, unknown.size, unknown.zoom), (1, 1, 1));
        // Corner, size and zoom buttons go through every value and back to the first.
        let mut s = read(&[("corner", "top_left"), ("size", "64"), ("zoom", "0.5")]);
        let mut seen = Vec::new();
        for _ in 0..5 {
            s.click(0);
            s.click(1);
            s.click(3);
            let stored = s.stored();
            seen.push((
                stored[0].1.clone(),
                stored[1].1.clone(),
                stored[3].1.clone(),
            ));
        }
        let row = |corner: &str, size: &str, zoom: &str| {
            (corner.to_owned(), size.to_owned(), zoom.to_owned())
        };
        assert_eq!(
            seen,
            [
                row("top_right", "96", "1"),
                row("bottom_left", "128", "2"),
                row("bottom_right", "64", "4"),
                row("top_left", "96", "0.5"),
                row("top_right", "128", "1"),
            ]
        );
    }
}
