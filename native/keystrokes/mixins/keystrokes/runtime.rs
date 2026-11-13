// Horizon's official Keystrokes, formerly the WebAssembly mod and before that
// a built-in extra: the movement keys, the mouse buttons and the space bar in
// a corner, lit while held, with clicks per second, drawn as the extra drew
// them. Its settings are the WebAssembly mod's files
// (`horizon-mods/.horizon-mod-data/keystrokes/corner`, `cps`), so a player
// keeps them. This file is both the mod's Cargo library (its tests) and the
// module the bundle adds to the game.

/// The mod's id (its settings folder), name and version.
pub const ID: &str = "keystrokes";
pub const NAME: &str = "Keystrokes";
pub const VERSION: &str = "2.0.0";
pub const DESCRIPTION: &str = "Your movement keys, mouse buttons and space bar in a corner, lit while held, with clicks per second. Horizon's own.";
/// The extra's colours: a key up is translucent black with white text, a key
/// held is light with dark text.
pub const BACKGROUND: u32 = 0x7800_0000;
pub const HELD: u32 = 0xB0FF_FFFF;
pub const INK_HELD: u32 = 0x14_1414;
pub const INK: u32 = 0xFF_FFFF;
/// A key's side and the gap between keys, in GUI pixels.
const KEY: f32 = 22.0;
const GAP: f32 = 2.0;
/// The mappings of the top two rows, in the order they are laid out.
pub const MOVES: [&str; 4] = ["key.forward", "key.left", "key.back", "key.right"];
/// The corners, as the settings files name them and the page labels them.
pub const CORNERS: [(&str, &str); 4] = [
    ("top_left", "Top left"),
    ("top_right", "Top right"),
    ("bottom_left", "Bottom left"),
    ("bottom_right", "Bottom right"),
];

/// The settings, as the WebAssembly Keystrokes kept them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Settings {
    /// Index in `CORNERS`.
    pub corner: usize,
    /// Clicks per second under the mouse buttons.
    pub cps: bool,
}

impl Settings {
    /// From the stored values; the extra's defaults (top left, clicks per
    /// second shown) for the ones missing.
    pub fn read(get: impl Fn(&str) -> Option<String>) -> Self {
        let corner = get("corner")
            .and_then(|c| CORNERS.iter().position(|k| k.0 == c.trim()))
            .unwrap_or(0);
        let cps = get("cps").is_none_or(|v| v.trim() != "false");
        Self { corner, cps }
    }
    pub fn labels(&self) -> Vec<String> {
        let on = |b: bool| if b { "ON" } else { "OFF" };
        vec![
            format!("Corner: {}", CORNERS[self.corner].1),
            format!("Clicks per second: {}", on(self.cps)),
        ]
    }
    pub fn click(&mut self, i: usize) {
        match i {
            0 => self.corner = (self.corner + 1) % CORNERS.len(),
            1 => self.cps = !self.cps,
            _ => {}
        }
    }
    pub fn stored(&self) -> [(&'static str, String); 2] {
        let cps = if self.cps { "true" } else { "false" };
        [
            ("corner", CORNERS[self.corner].0.to_owned()),
            ("cps", cps.to_owned()),
        ]
    }
}

/// What the box shows: each movement key (held, label), the space bar, the
/// mouse buttons, and the clicks per second when shown.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    pub moves: [(bool, String); 4],
    pub jump: bool,
    pub attack: bool,
    pub use_: bool,
    pub cps: Option<(u32, u32)>,
}

/// One thing to draw, relative to the box's top left corner.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    /// x, y, w, h and argb.
    Fill([f32; 4], u32),
    /// The text, its x and y, rgb, and a shadow.
    Text(String, [f32; 2], u32, bool),
}

/// The box's size.
pub const SIZE: (f32, f32) = (3.0 * KEY + 2.0 * GAP, 3.0 * KEY + 3.0 * GAP + 10.0);

/// A key's box with its lines centred; `width` measures text.
fn key_box(
    out: &mut Vec<Shape>,
    [x, y, w, h]: [f32; 4],
    held: bool,
    lines: &[&str],
    width: &dyn Fn(&str) -> f32,
) {
    let (fill, ink) = if held {
        (HELD, INK_HELD)
    } else {
        (BACKGROUND, INK)
    };
    out.push(Shape::Fill([x, y, w, h], fill));
    let total = lines.len() as f32 * 9.0 - 1.0;
    for (i, line) in lines.iter().enumerate() {
        let at = [
            (x + (w - width(line)) / 2.0).round(),
            (y + (h - total) / 2.0).round() + i as f32 * 9.0,
        ];
        out.push(Shape::Text((*line).to_owned(), at, ink, !held));
    }
}

/// The box, as the extra laid it out: W, then A S D, then the mouse buttons
/// with their clicks per second, then the space bar.
pub fn shapes(v: &View, width: &dyn Fn(&str) -> f32) -> Vec<Shape> {
    let (w, _) = SIZE;
    let step = KEY + GAP;
    let mut out = Vec::with_capacity(24);
    let [forward, left, back, right] = &v.moves;
    key_box(
        &mut out,
        [step, 0.0, KEY, KEY],
        forward.0,
        &[&forward.1],
        width,
    );
    for (i, key) in [left, back, right].into_iter().enumerate() {
        key_box(
            &mut out,
            [i as f32 * step, step, KEY, KEY],
            key.0,
            &[&key.1],
            width,
        );
    }
    let mw = (w - GAP) / 2.0;
    let my = 2.0 * step;
    let buttons = [
        (v.attack, "LMB", v.cps.map(|c| c.0)),
        (v.use_, "RMB", v.cps.map(|c| c.1)),
    ];
    for (i, (held, label, cps)) in buttons.into_iter().enumerate() {
        let rate = cps.map(|n| format!("{n} CPS"));
        let lines: Vec<&str> = std::iter::once(label).chain(rate.as_deref()).collect();
        key_box(
            &mut out,
            [i as f32 * (mw + GAP), my, mw, KEY],
            held,
            &lines,
            width,
        );
    }
    let sy = my + step;
    key_box(&mut out, [0.0, sy, w, 10.0], v.jump, &[], width);
    let bar = 0xFF00_0000 | if v.jump { INK_HELD } else { INK };
    out.push(Shape::Fill([w / 2.0 - 12.0, sy + 4.0, 24.0, 2.0], bar));
    out
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
    fn reads_the_webassembly_mods_settings() {
        let none = |_: &str| None;
        assert_eq!(
            Settings::read(none),
            Settings {
                corner: 0,
                cps: true
            }
        );
        let get = |k: &str| (k == "cps").then(|| "false".to_owned());
        let mut s = Settings::read(get);
        assert!(!s.cps);
        s.click(0);
        assert_eq!(s.labels(), ["Corner: Top right", "Clicks per second: OFF"]);
        assert_eq!(s.stored()[0], ("corner", "top_right".to_owned()));
    }

    #[test]
    fn lays_the_keys_out_as_the_extra_did() {
        let mut v = View {
            moves: ["W", "A", "S", "D"].map(|k| (false, k.to_owned())),
            cps: Some((3, 0)),
            ..View::default()
        };
        v.moves[0].0 = true;
        let width = |t: &str| t.len() as f32 * 6.0;
        let out = shapes(&v, &width);
        // W lit: light fill, dark text with no shadow, centred in its key.
        assert_eq!(out[0], Shape::Fill([24.0, 0.0, 22.0, 22.0], HELD));
        assert_eq!(
            out[1],
            Shape::Text("W".into(), [32.0, 7.0], INK_HELD, false)
        );
        assert!(out.contains(&Shape::Text("3 CPS".into(), [2.0, 60.0], INK, true)));
        assert_eq!(SIZE, (70.0, 82.0));
    }

    #[test]
    fn the_stored_files_of_earlier_versions_are_read_back() {
        // Each corner is read from the name the earlier versions' file holds.
        for (i, (name, _)) in CORNERS.iter().enumerate() {
            let get = |k: &str| (k == "corner").then(|| format!("{name}\n"));
            assert_eq!(Settings::read(get).corner, i, "{name}");
        }
        // An earlier version's bottom right with the clicks per second off.
        let get = |k: &str| match k {
            "corner" => Some("bottom_right".to_owned()),
            "cps" => Some("false".to_owned()),
            _ => None,
        };
        let s = Settings::read(get);
        assert_eq!(
            s,
            Settings {
                corner: 3,
                cps: false
            }
        );
        assert_eq!(
            s.stored(),
            [
                ("corner", "bottom_right".to_owned()),
                ("cps", "false".to_owned())
            ]
        );
        // A corner it does not know, and any cps but "false", are the extra's defaults.
        let odd = |k: &str| Some(if k == "corner" { "middle" } else { "yes" }.to_owned());
        assert_eq!(
            Settings::read(odd),
            Settings {
                corner: 0,
                cps: true
            }
        );
        // The Corner button goes round the four corners and back to the first.
        let mut s = Settings {
            corner: 0,
            cps: true,
        };
        let seen: Vec<String> = (0..5)
            .map(|_| {
                s.click(0);
                s.stored()[0].1.clone()
            })
            .collect();
        assert_eq!(
            seen,
            [
                "top_right",
                "bottom_left",
                "bottom_right",
                "top_left",
                "top_right"
            ]
        );
        assert!(
            s.cps,
            "the Corner button leaves the clicks per second alone"
        );
    }
}
