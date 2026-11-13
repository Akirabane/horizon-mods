// Horizon's Coordinates HUD, formerly the WebAssembly SDK's example mod: the
// block position and the direction the player faces in a box in the top-left
// corner, `/where` to put the position in chat and `/coords` to hide or show
// the box. This file is both the mod's Cargo library (its tests) and the
// module the bundle adds to the game.

/// The mod's id (its settings folder), name and version.
pub const ID: &str = "coords_hud";
pub const NAME: &str = "Coordinates HUD";
pub const VERSION: &str = "2.0.0";
pub const DESCRIPTION: &str = "Your coordinates and facing in a corner of the screen. /where puts them in chat, /coords hides or shows the box.";
/// The WebAssembly mods' panel: translucent black, white text with a shadow.
pub const BACKGROUND: u32 = 0x7800_0000;
pub const INK: u32 = 0xFF_FFFF;

/// Direction.fromYRot: yaw 0 faces south, 90 west.
pub fn facing(yaw: f32) -> &'static str {
    let i = (yaw / 90.0 + 0.5).floor() as i32 & 3;
    ["South (+Z)", "West (-X)", "North (-Z)", "East (+X)"][i as usize]
}

/// The box's lines, as the WebAssembly mod wrote them.
pub fn lines(x: f64, y: f64, z: f64, yaw: f32) -> [String; 2] {
    [
        format!("XYZ {x:.1} / {y:.1} / {z:.1}"),
        format!("Facing {}", facing(yaw)),
    ]
}

/// `/where`'s line: `12, 70, -4 in minecraft:the_nether`.
pub fn where_line(x: f64, y: f64, z: f64, dimension: &str) -> String {
    format!(
        "{:.0}, {:.0}, {:.0} in {dimension}",
        x.floor(),
        y.floor(),
        z.floor()
    )
}

/// The stored `shown` value: shown unless it says `false`.
pub fn read_shown(stored: Option<String>) -> bool {
    stored.is_none_or(|v| v.trim() != "false")
}

pub fn shown_label(shown: bool) -> String {
    format!("Shown: {}", if shown { "ON" } else { "OFF" })
}

thread_local! {
    /// Whether the box shows, read once from its file, then kept.
    static SHOWN: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Whether the box shows, read once through `get` (the `shown` file).
pub fn shown(get: impl FnOnce() -> Option<String>) -> bool {
    SHOWN.with(|s| {
        let found = s.get().unwrap_or_else(|| read_shown(get()));
        s.set(Some(found));
        found
    })
}

/// Replaces it (`/coords` or the settings page changed it).
pub fn set_shown(shown: bool) {
    SHOWN.with(|s| s.set(Some(shown)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facing_follows_vanilla_yaw() {
        assert_eq!(facing(0.0), "South (+Z)");
        assert_eq!(facing(90.0), "West (-X)");
        assert_eq!(facing(-180.0), "North (-Z)");
        assert_eq!(facing(-90.0), "East (+X)");
        assert_eq!(facing(314.0), "East (+X)");
    }

    #[test]
    fn facing_turns_at_the_half_way_yaws() {
        // Direction.fromYRot: each side owns 90 degrees centred on its own yaw, so it
        // changes half way (45, 135, 225, 315), the half way yaw itself going to the next.
        let south = "South (+Z)";
        let (west, north, east) = ("West (-X)", "North (-Z)", "East (+X)");
        for (yaw, side) in [
            (44.9, south),
            (45.0, west),
            (134.9, west),
            (135.0, north),
            (224.9, north),
            (225.0, east),
            (314.9, east),
            (315.0, south),
            (-44.9, south),
            (-45.0, south),
            (-45.1, east),
            (-135.0, east),
            (-135.1, north),
        ] {
            assert_eq!(facing(yaw), side, "yaw {yaw}");
        }
    }

    #[test]
    fn lines_read_as_the_webassembly_mods() {
        assert_eq!(
            lines(12.5, 64.0, -3.25, 180.0),
            ["XYZ 12.5 / 64.0 / -3.2", "Facing North (-Z)"]
        );
        assert_eq!(
            where_line(12.5, 70.2, -3.25, "minecraft:the_nether"),
            "12, 70, -4 in minecraft:the_nether"
        );
    }

    #[test]
    fn shown_unless_stored_false() {
        assert!(read_shown(None));
        assert!(read_shown(Some("true".into())));
        assert!(!read_shown(Some("false\n".into())));
        assert_eq!(shown_label(false), "Shown: OFF");
    }
}
