//! The user's configuration file: `$XDG_CONFIG_HOME/lofi/config.toml`,
//! falling back to `$HOME/.config/lofi/config.toml`.
//!
//! ## Why LoFi needs one at all
//!
//! Under GNOME the launcher is an ordinary toplevel, so Mutter and GTK's
//! client-side decorations give it the system's window shadow and corner
//! rounding for free. Under Niri it is a `wlr-layer-shell` overlay surface,
//! and that path has neither:
//!
//! - `libgtk4-layer-shell` calls `gtk_window_set_decorated(FALSE)` on the
//!   window it converts, so there is no CSD decoration node to draw a shadow
//!   or round the corners. The surface is a bare rectangle.
//! - Niri's `layer-rule` can draw a **shadow** for a layer surface, and
//!   `geometry-corner-radius` will round *that shadow* — but it cannot round
//!   the surface itself (`clip-to-geometry` is window-rule-only) and it has no
//!   `border` at all. Those two are the client's job, and only the client can
//!   do them.
//!
//! So the launcher has to be told what the user's Niri looks like. It is told
//! rather than deduced: LoFi deliberately does **not** read Niri's own
//! `config.kdl`. That file is the compositor's, its schema is Niri's to change,
//! and the values LoFi would want (`layout.border`, a window rule's
//! `geometry-corner-radius`) are per-window-rule anyway, so there is no single
//! correct answer to read out of it. A handful of numbers the user writes down
//! once is both simpler and predictable, which is the launcher's stated goal.
//!
//! ## Why TOML
//!
//! `serde` is already a dependency and `toml` is already in the lock file (the
//! gtk4-sys build scripts pull it in through `system-deps`), so the format
//! costs one line. KDL would let the `[appearance.shadow]` block be a literal
//! copy-paste out of `config.kdl`, but it would add a parser this crate
//! otherwise has no use for, and the values are four scalars either way.
//!
//! The *names* still mirror Niri's, because Niri defines its shadow
//! parameters by reference to CSS: "`softness` … same as CSS box-shadow blur
//! radius", "`spread` … same as CSS box-shadow spread", "`offset` … same as
//! CSS box-shadow offset". LoFi renders the shadow as a GTK CSS `box-shadow`,
//! so the translation is exact and a user copies the numbers straight across.
//! The defaults below are Niri's own defaults for the same reason: an empty
//! `[appearance.shadow]` block gives Niri's stock shadow.
//!
//! ## Error policy
//!
//! Same as the rest of the crate: log to stderr and degrade. A missing file is
//! not an error at all (it is the common case); an unreadable or malformed one
//! logs and leaves the launcher on defaults. A launcher that refuses to open
//! because of a typo in a cosmetic setting would be a much worse outcome than
//! one that opens looking plain.

use std::env;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer};

/// Corner radius applied when the file says nothing. Matches libadwaita's own
/// window radius, so an unconfigured Niri session comes up looking close to
/// what the same launcher already gets on GNOME rather than as a hard-cornered
/// rectangle.
pub const DEFAULT_CORNER_RADIUS: u32 = 12;

/// Border colour applied when a width is set but no colour is. A GTK named
/// colour rather than a literal, for the same reason the rest of
/// `ui::LAUNCHER_CSS` names them: it follows the light/dark scheme.
pub const DEFAULT_BORDER_COLOR: &str = "@borders";

/// Shadow colour applied when the block omits it. Niri's own default.
pub const DEFAULT_SHADOW_COLOR: &str = "#0007";

/// Niri's default shadow geometry, used for any key the block omits.
const DEFAULT_SHADOW_SOFTNESS: u32 = 30;
const DEFAULT_SHADOW_SPREAD: u32 = 5;
const DEFAULT_SHADOW_OFFSET_Y: i32 = 5;

/// Upper bound on the transparent margin a shadow may add to each side of the
/// window, in logical pixels.
///
/// The margin is not cosmetic: it grows the layer surface itself (see
/// [`Shadow::margin`]), so a fat-fingered `softness = 3000` would ask the
/// compositor for a surface several times the size of the screen. Every other
/// setting here is clamped by GTK on its own — an absurd `corner-radius` or
/// `border-width` just eats the window and is immediately obvious — so this is
/// the one value worth a guard.
const MAX_SHADOW_MARGIN: u32 = 256;

/// The whole configuration file. Every field is optional; the `Default` impl
/// is what an absent file means.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Config {
    /// Shell command line the "Lock" entry runs under Niri, below
    /// `$LOFI_LOCK_COMMAND` and above the `$PATH` search. See
    /// `backend::niri::power::lock` for the full chain.
    pub lock_command: Option<String>,

    /// How the launcher window is drawn. Only consulted on the layer-shell
    /// path — see the module docs.
    #[serde(default)]
    pub appearance: Appearance,
}

/// Border, corner rounding, and optional drop shadow for the launcher window.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Appearance {
    /// Corner radius in logical pixels. `0` gives square corners.
    #[serde(default = "default_corner_radius")]
    pub corner_radius: u32,

    /// Border width in logical pixels. `0` — the default — means no border is
    /// drawn at all, and `border_color` is inert.
    #[serde(default)]
    pub border_width: u32,

    /// Border colour. Ignored unless `border_width` is non-zero.
    #[serde(default = "default_border_color")]
    pub border_color: Color,

    /// Drop shadow LoFi draws itself. Absent — the default — means the surface
    /// stays exactly the visible rectangle, which is the shape Niri's own
    /// `layer-rule { shadow { on } }` needs (it draws its shadow around the
    /// whole buffer, invisible margins included, because a layer surface has
    /// no `xdg_surface.set_window_geometry` to declare its visual bounds).
    /// Leaving this out is therefore how a user opts into the compositor
    /// drawing the shadow instead.
    pub shadow: Option<Shadow>,
}

impl Default for Appearance {
    fn default() -> Self {
        Appearance {
            corner_radius: DEFAULT_CORNER_RADIUS,
            border_width: 0,
            border_color: default_border_color(),
            shadow: None,
        }
    }
}

/// A drop shadow, in Niri's vocabulary. Each field maps 1:1 onto the CSS
/// `box-shadow` component of the same meaning; see the module docs.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Shadow {
    /// Blur radius in logical pixels. `0` gives a hard shadow.
    #[serde(default = "default_shadow_softness")]
    pub softness: u32,

    /// Distance the shadow rectangle is expanded past the window, in logical
    /// pixels.
    #[serde(default = "default_shadow_spread")]
    pub spread: u32,

    /// Displacement of the shadow relative to the window.
    #[serde(default = "default_shadow_offset")]
    pub offset: Offset,

    /// Colour and opacity.
    #[serde(default = "default_shadow_color")]
    pub color: Color,
}

impl Default for Shadow {
    fn default() -> Self {
        Shadow {
            softness: DEFAULT_SHADOW_SOFTNESS,
            spread: DEFAULT_SHADOW_SPREAD,
            offset: default_shadow_offset(),
            color: default_shadow_color(),
        }
    }
}

impl Shadow {
    /// Transparent margin the window needs on **every** side to have room to
    /// draw this shadow, in logical pixels.
    ///
    /// Symmetric, and derived from the largest of the two offset components,
    /// rather than tight per-edge margins. The launcher's layer surface is
    /// anchored to no edge, which is what makes Niri centre it — so the
    /// *surface* is what gets centred, and an asymmetric margin would shift
    /// the visible window off-centre by half the difference. Paying a few
    /// transparent pixels on two sides is the cheaper trade.
    ///
    /// Clamped to [`MAX_SHADOW_MARGIN`]; saturating arithmetic throughout so a
    /// nonsense value in the file can't overflow into a small margin.
    pub fn margin(&self) -> i32 {
        let reach = self.softness.saturating_add(self.spread);
        let offset = self
            .offset
            .x
            .unsigned_abs()
            .max(self.offset.y.unsigned_abs());
        let margin = reach.saturating_add(offset).min(MAX_SHADOW_MARGIN);
        // In range by construction: MAX_SHADOW_MARGIN is far below i32::MAX.
        i32::try_from(margin).unwrap_or(i32::MAX)
    }
}

/// Shadow displacement, in logical pixels. Positive `y` is downwards and
/// positive `x` is rightwards, matching both Niri and CSS.
///
/// Note the two different "defaults" in play. An `offset` key the user did not
/// write at all falls back to [`default_shadow_offset`] — Niri's `x=0 y=5` —
/// so that a bare `[appearance.shadow]` block reproduces Niri's stock shadow.
/// A component missing from an `offset` table the user *did* write falls back
/// to `0`, because `offset = { x = 5 }` plainly means "shift it right", not
/// "shift it right and also down by five".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Offset {
    #[serde(default)]
    pub x: i32,
    #[serde(default)]
    pub y: i32,
}

/// A colour destined for a GTK stylesheet.
///
/// This is a newtype with a validating `Deserialize` rather than a bare
/// `String` because the value is **interpolated into a CSS string** by
/// `ui::launcher_css`. A raw pass-through would let a config file close the
/// declaration and open its own — `#fff; } * { background-image: url(…)` —
/// which is CSS injection from a file LoFi reads without being asked to. An
/// allowlist of the three shapes a colour can actually take is both the fix
/// and, conveniently, a typo check: a malformed colour is reported by name
/// instead of being silently dropped by GTK's own parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Color(String);

impl Color {
    /// Validate `raw` as one of the three accepted shapes:
    ///
    /// 1. `#` plus 3, 4, 6, or 8 hex digits (`#0007`, `#00000080`) — what both
    ///    Niri's config and CSS Color 4 use.
    /// 2. `@` plus a GTK/libadwaita named colour (`@borders`,
    ///    `@accent_color`), which is how the rest of the launcher's stylesheet
    ///    stays light/dark aware.
    /// 3. A bare CSS colour keyword (`black`, `rebeccapurple`).
    ///
    /// Note what all three have in common: no whitespace, no punctuation, and
    /// in particular no `;`, `}`, `(`, or `/`. That is the property being
    /// enforced, not the exhaustiveness of the keyword list — an unknown but
    /// well-shaped keyword is left for GTK to reject on its own terms.
    pub fn parse(raw: &str) -> Result<Color, String> {
        let value = raw.trim();

        if let Some(digits) = value.strip_prefix('#') {
            if matches!(digits.len(), 3 | 4 | 6 | 8)
                && digits.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Ok(Color(value.to_owned()));
            }
            return Err(format!(
                "{raw:?} is not a hex colour: expected '#' followed by 3, 4, 6, or 8 hex digits"
            ));
        }

        if let Some(name) = value.strip_prefix('@') {
            if !name.is_empty()
                && name.len() <= 64
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            {
                return Ok(Color(value.to_owned()));
            }
            return Err(format!(
                "{raw:?} is not a named GTK colour: expected '@' followed by letters, \
                 digits, '_', or '-'"
            ));
        }

        if !value.is_empty() && value.len() <= 32 && value.bytes().all(|b| b.is_ascii_alphabetic())
        {
            return Ok(Color(value.to_owned()));
        }

        Err(format!(
            "{raw:?} is not a colour: expected a hex colour (#0007), a GTK named colour \
             (@borders), or a CSS keyword (black)"
        ))
    }

    /// The validated text, ready to be written into a CSS declaration.
    pub fn as_css(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Color::parse(&raw).map_err(serde::de::Error::custom)
    }
}

fn default_corner_radius() -> u32 {
    DEFAULT_CORNER_RADIUS
}

fn default_border_color() -> Color {
    Color(DEFAULT_BORDER_COLOR.to_owned())
}

fn default_shadow_color() -> Color {
    Color(DEFAULT_SHADOW_COLOR.to_owned())
}

fn default_shadow_softness() -> u32 {
    DEFAULT_SHADOW_SOFTNESS
}

fn default_shadow_spread() -> u32 {
    DEFAULT_SHADOW_SPREAD
}

fn default_shadow_offset() -> Offset {
    Offset {
        x: 0,
        y: DEFAULT_SHADOW_OFFSET_Y,
    }
}

/// Parse a configuration file's text.
///
/// Pure and separate from [`load`] for the same reason
/// `apps::gather_applications` takes its directories as a parameter and
/// `backend::detect` takes its environment values: it makes the interesting
/// half unit-testable without touching the filesystem or process environment.
pub fn parse(text: &str) -> Result<Config, toml::de::Error> {
    toml::from_str(text)
}

/// Resolve the configuration file's path, preferring `$XDG_CONFIG_HOME` and
/// falling back to `$HOME/.config`. `None` when neither variable resolves, in
/// which case the launcher runs on defaults.
///
/// Mirrors the manual XDG pattern in `apps::application_directories` and
/// `main::mru_state_path` rather than pulling in a dirs crate, for consistency
/// with them.
pub fn path() -> Option<PathBuf> {
    let config_home: PathBuf = match env::var("XDG_CONFIG_HOME") {
        Ok(value) if !value.is_empty() => PathBuf::from(value),
        _ => match env::var("HOME") {
            Ok(home) if !home.is_empty() => {
                let mut p = PathBuf::from(home);
                p.push(".config");
                p
            }
            _ => return None,
        },
    };
    let mut path = config_home;
    path.push("lofi");
    path.push("config.toml");
    Some(path)
}

/// Read and parse the configuration file at `path`.
///
/// A missing file yields the defaults **silently** — that is the common case,
/// not a problem to report. Anything else (unreadable, malformed) logs to
/// stderr and yields the defaults, so a broken file costs the user their
/// styling rather than their launcher.
pub fn load_from(path: &Path) -> Config {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Config::default(),
        Err(e) => {
            eprintln!("config: {} could not be read: {e}", path.display());
            return Config::default();
        }
    };

    match parse(&text) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("config: {} is not valid: {e}", path.display());
            Config::default()
        }
    }
}

/// [`load_from`] against the path [`path`] resolves. Defaults when there is no
/// resolvable path at all.
pub fn load() -> Config {
    match path() {
        Some(path) => load_from(&path),
        None => Config::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_file_is_the_defaults() {
        // The common case is an absent file, but an empty or comment-only one
        // must land in the same place rather than erroring.
        let config = parse("").expect("an empty file should parse");
        assert_eq!(config, Config::default());
        assert_eq!(config.appearance.corner_radius, DEFAULT_CORNER_RADIUS);
        assert_eq!(config.appearance.border_width, 0);
        assert_eq!(
            config.appearance.shadow, None,
            "no shadow unless the file asks for one — an absent shadow is what \
             leaves the surface the right shape for Niri to draw its own"
        );
    }

    #[test]
    fn a_full_file_round_trips() {
        let config = parse(
            r##"
lock-command = "swaylock -f -c 000000"

[appearance]
corner-radius = 16
border-width = 3
border-color = "#e0e0e0"

[appearance.shadow]
softness = 30
spread = 8
offset = { x = 5, y = 5 }
color = "#00000080"
"##,
        )
        .expect("the documented example should parse");

        assert_eq!(
            config.lock_command.as_deref(),
            Some("swaylock -f -c 000000")
        );
        assert_eq!(config.appearance.corner_radius, 16);
        assert_eq!(config.appearance.border_width, 3);
        assert_eq!(config.appearance.border_color.as_css(), "#e0e0e0");

        let shadow = config.appearance.shadow.expect("shadow should be present");
        assert_eq!(shadow.softness, 30);
        assert_eq!(shadow.spread, 8);
        assert_eq!(shadow.offset, Offset { x: 5, y: 5 });
        assert_eq!(shadow.color.as_css(), "#00000080");
    }

    #[test]
    fn the_home_manager_modules_output_shape_parses() {
        // `pkgs.formats.toml` renders a nested table as its own `[a.b.c]`
        // header rather than as an inline table, so the file the module
        // generates does not look like the hand-written one above. Both are
        // the same TOML data model and serde treats them identically — this
        // pins that, because nothing else would catch the module and the
        // parser drifting apart.
        let config = parse(
            r##"
lock-command = "hyprlock"

[appearance]
border-color = "#e0e0e0"
border-width = 3
corner-radius = 12

[appearance.shadow]
color = "#00000080"
softness = 30
spread = 8

[appearance.shadow.offset]
y = 5
"##,
        )
        .expect("the home-manager module's output should parse");

        let shadow = config.appearance.shadow.expect("shadow should be present");
        assert_eq!(
            shadow.offset,
            Offset { x: 0, y: 5 },
            "a half-written offset table pins the other component to 0, which \
             is what the module's offsetX/offsetY options document"
        );
        assert_eq!(config.appearance.border_width, 3);
        assert_eq!(config.lock_command.as_deref(), Some("hyprlock"));
    }

    #[test]
    fn an_empty_shadow_block_is_niris_default_shadow() {
        // The point of mirroring Niri's key names and defaults: writing the
        // block and nothing else should give Niri's stock shadow.
        let config = parse("[appearance.shadow]\n").expect("a bare shadow block should parse");
        assert_eq!(
            config.appearance.shadow,
            Some(Shadow::default()),
            "an empty [appearance.shadow] should be Niri's own defaults"
        );
        let shadow = config.appearance.shadow.expect("shadow should be present");
        assert_eq!(shadow.softness, 30);
        assert_eq!(shadow.spread, 5);
        assert_eq!(shadow.offset, Offset { x: 0, y: 5 });
        assert_eq!(shadow.color.as_css(), "#0007");
    }

    #[test]
    fn an_unknown_key_is_an_error_not_a_shrug() {
        // `deny_unknown_fields` is deliberate: silently ignoring a misspelled
        // `border-colour` would leave the user staring at an unchanged window
        // with nothing to go on.
        let err = parse("[appearance]\nborder-colour = \"#fff\"\n")
            .expect_err("a misspelled key should be reported");
        assert!(
            err.to_string().contains("border-colour"),
            "the error should name the offending key, got: {err}"
        );
    }

    #[test]
    fn colours_that_could_escape_the_declaration_are_rejected() {
        // The value is interpolated into a stylesheet, so this is the security
        // property, not a style preference.
        for hostile in [
            "#fff; } * { background-image: url(http://example.com/x.png)",
            "red; }",
            "url(/etc/passwd)",
            "rgb(1,2,3)",
            "/* */",
            "",
            "   ",
        ] {
            assert!(
                Color::parse(hostile).is_err(),
                "{hostile:?} must not reach the stylesheet"
            );
        }
    }

    #[test]
    fn colours_accept_the_three_documented_shapes() {
        for good in [
            "#fff",
            "#0007",
            "#e0e0e0",
            "#00000080",
            "@borders",
            "@popover_bg_color",
            "black",
            "rebeccapurple",
        ] {
            let color = Color::parse(good).unwrap_or_else(|e| panic!("{good:?} should parse: {e}"));
            assert_eq!(color.as_css(), good);
        }

        // Surrounding whitespace is the user's, not the stylesheet's.
        assert_eq!(
            Color::parse("  #fff  ")
                .expect("padded colour should parse")
                .as_css(),
            "#fff"
        );

        // A hex colour of the wrong length is a typo worth reporting, not a
        // value to hand to GTK.
        assert!(Color::parse("#ff").is_err(), "2 hex digits is not a colour");
        assert!(
            Color::parse("#fffff").is_err(),
            "5 hex digits is not a colour"
        );
        assert!(
            Color::parse("#gggg").is_err(),
            "non-hex digits are not a colour"
        );
    }

    #[test]
    fn a_bad_colour_fails_the_parse_and_names_itself() {
        let err = parse("[appearance]\nborder-color = \"not a colour\"\n")
            .expect_err("an invalid colour should fail the parse");
        assert!(
            err.to_string().contains("not a colour"),
            "the error should quote the offending value, got: {err}"
        );
    }

    #[test]
    fn shadow_margin_covers_blur_spread_and_offset() {
        let shadow = Shadow {
            softness: 30,
            spread: 8,
            offset: Offset { x: 5, y: 2 },
            color: default_shadow_color(),
        };
        assert_eq!(
            shadow.margin(),
            43,
            "the margin must cover softness + spread + the larger offset component"
        );

        // Symmetric by construction: the larger component wins on both axes,
        // because the surface — not the visible window — is what the
        // compositor centres.
        let mirrored = Shadow {
            offset: Offset { x: 2, y: 5 },
            ..shadow.clone()
        };
        assert_eq!(mirrored.margin(), shadow.margin());

        // A negative offset reaches just as far as a positive one.
        let negative = Shadow {
            offset: Offset { x: -5, y: 0 },
            ..shadow.clone()
        };
        assert_eq!(negative.margin(), 43);
    }

    #[test]
    fn shadow_margin_is_clamped_and_cannot_overflow() {
        // The margin grows the layer surface itself, so a nonsense value must
        // clamp rather than ask the compositor for a surface the size of a
        // city block — or, worse, wrap into a small one.
        let absurd = Shadow {
            softness: u32::MAX,
            spread: u32::MAX,
            offset: Offset {
                x: i32::MIN,
                y: i32::MAX,
            },
            color: default_shadow_color(),
        };
        assert_eq!(
            absurd.margin(),
            i32::try_from(MAX_SHADOW_MARGIN).expect("the cap fits in an i32"),
            "saturating arithmetic plus the cap should survive the extremes"
        );
    }

    #[test]
    fn a_missing_file_is_the_defaults_and_not_an_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(
            load_from(&dir.path().join("does-not-exist.toml")),
            Config::default(),
            "an absent config file is the common case, not a failure"
        );
    }

    #[test]
    fn a_malformed_file_degrades_to_the_defaults() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "this is not [ toml").expect("fixture should be writable");
        assert_eq!(
            load_from(&path),
            Config::default(),
            "a broken file should cost the user their styling, not their launcher"
        );
    }

    #[test]
    fn a_file_on_disk_is_read() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[appearance]\nborder-width = 2\n")
            .expect("fixture should be writable");
        assert_eq!(load_from(&path).appearance.border_width, 2);
    }
}
