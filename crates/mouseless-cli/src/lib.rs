//! `mouseless-cli` library: verbs shared by the binary and deep links.
//!
//! Parity requirement (plan §4): every `mouseless://` deep link maps 1:1 to a
//! CLI verb (`mouseless://toggle-overlay` ↔ `mouseless toggle-overlay`).

use thiserror::Error;

use mouseless_kbd::{BASE_LAYER, OVERLAY_LAYER};

/// CLI errors (surfaced with a clear message, never a stack trace).
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CliError {
    /// Unknown verb or bad arguments.
    #[error("usage: mouseless <show|hide|toggle|status|reload|settings|--list-input-devices>")]
    Usage,
}

/// Settings pages reachable through `mouseless://settings/<page>`.
///
/// The GUI uses the same parser so the CLI, the `.desktop` handler, and the
/// Adwaita window cannot drift apart. `Grid` is the landing page and accepts
/// both `mouseless://settings` and `mouseless://settings/grid`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsPage {
    /// Grid keys, subgrid, monitor assignment.
    Grid,
    /// Colors, opacity, cursor hiding.
    Appearance,
    /// Action level, thresholds, click/drag modifiers.
    Behavior,
    /// Layer names, key roles, kill switch.
    Keybindings,
    /// Built-in and user presets.
    Presets,
    /// Live daemon/overlay/kanata state.
    Status,
}

impl SettingsPage {
    /// Every page in window order.
    pub const ALL: [SettingsPage; 6] = [
        SettingsPage::Grid,
        SettingsPage::Appearance,
        SettingsPage::Behavior,
        SettingsPage::Keybindings,
        SettingsPage::Presets,
        SettingsPage::Status,
    ];

    /// Stable page name used by the GUI stack and deep links.
    pub fn name(self) -> &'static str {
        match self {
            SettingsPage::Grid => "grid",
            SettingsPage::Appearance => "appearance",
            SettingsPage::Behavior => "behavior",
            SettingsPage::Keybindings => "keybindings",
            SettingsPage::Presets => "presets",
            SettingsPage::Status => "status",
        }
    }

    /// Human-readable page title.
    pub fn title(self) -> &'static str {
        match self {
            SettingsPage::Grid => "Grid",
            SettingsPage::Appearance => "Appearance",
            SettingsPage::Behavior => "Behavior",
            SettingsPage::Keybindings => "Keybindings",
            SettingsPage::Presets => "Presets",
            SettingsPage::Status => "Status",
        }
    }

    /// Canonical deep link for this page (default `mouseless` scheme).
    pub fn deep_link(self) -> &'static str {
        match self {
            SettingsPage::Grid => "mouseless://settings",
            SettingsPage::Appearance => "mouseless://settings/appearance",
            SettingsPage::Behavior => "mouseless://settings/behavior",
            SettingsPage::Keybindings => "mouseless://settings/keybindings",
            SettingsPage::Presets => "mouseless://settings/presets",
            SettingsPage::Status => "mouseless://settings/status",
        }
    }

    /// Path component shared by every scheme.
    pub fn path(self) -> &'static str {
        match self {
            SettingsPage::Grid => "settings",
            SettingsPage::Appearance => "settings/appearance",
            SettingsPage::Behavior => "settings/behavior",
            SettingsPage::Keybindings => "settings/keybindings",
            SettingsPage::Presets => "settings/presets",
            SettingsPage::Status => "settings/status",
        }
    }

    /// Canonical deep link using a configured URL scheme.
    pub fn deep_link_with_scheme(self, scheme: &str) -> String {
        format!("{scheme}://{}", self.path())
    }

    /// Parse a settings deep link (or a bare page name).
    pub fn parse(value: &str) -> Option<Self> {
        Self::parse_with_scheme(value, "mouseless")
    }

    /// Parse a settings deep link using a configured URL scheme.
    pub fn parse_with_scheme(value: &str, scheme: &str) -> Option<Self> {
        let prefix = format!("{scheme}://");
        let path = value.strip_prefix(&prefix).unwrap_or(value);
        let path = path.strip_suffix('/').unwrap_or(path);
        match path {
            "settings" | "settings/grid" | "grid" => Some(SettingsPage::Grid),
            "settings/appearance" | "appearance" => Some(SettingsPage::Appearance),
            "settings/behavior" | "behavior" => Some(SettingsPage::Behavior),
            "settings/keybindings" | "keybindings" => Some(SettingsPage::Keybindings),
            "settings/presets" | "presets" => Some(SettingsPage::Presets),
            "settings/status" | "status" => Some(SettingsPage::Status),
            _ => None,
        }
    }
}

/// Kanata layer names used by CLI verb mapping.
///
/// `base` comes from `mouseless.toml`; the overlay name comes from the user
/// settings (`config.yaml`) and keeps its pinned default here for verbs that
/// run without a config document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayerNames<'a> {
    /// Neutral layer restored on hide/exit.
    pub base: &'a str,
    /// Overlay layer.
    pub overlay: &'a str,
}

impl Default for LayerNames<'static> {
    fn default() -> Self {
        Self {
            base: BASE_LAYER,
            overlay: OVERLAY_LAYER,
        }
    }
}

/// Verbs with exact deep-link parity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    /// `mouseless://show-overlay`.
    Show,
    /// `mouseless://hide-overlay`.
    Hide,
    /// `mouseless://toggle-overlay`.
    Toggle,
    /// Kanata layer plus daemon/overlay/session state.
    Status,
    /// Re-read the daemon-owned config, validate, and apply it.
    Reload,
    /// List `/dev/input` devices (mirrors upstream `--list-input-devices`).
    ListInputDevices,
    /// Open the settings GUI at a page (`mouseless://settings/...`).
    Settings(SettingsPage),
}

impl Verb {
    /// Parse `argv[1]` into a verb.
    ///
    /// Both the plain verb and its `mouseless://` deep link are accepted, so
    /// one `.desktop` scheme handler can forward URLs straight to the CLI
    /// (SPEC-cli parity).
    pub fn parse(arg: &str) -> Result<Self, CliError> {
        Self::parse_with_scheme(arg, "mouseless")
    }

    /// Parse `argv[1]` using a configured URL scheme.
    pub fn parse_with_scheme(arg: &str, scheme: &str) -> Result<Self, CliError> {
        let prefix = format!("{scheme}://");
        if arg.starts_with(&prefix) {
            return Self::from_deep_link_with_scheme(arg, scheme).ok_or(CliError::Usage);
        }
        if arg == "settings" || arg.starts_with("settings/") {
            return SettingsPage::parse_with_scheme(arg, scheme)
                .map(Verb::Settings)
                .ok_or(CliError::Usage);
        }
        match arg {
            "show" | "show-overlay" => Ok(Verb::Show),
            "hide" | "hide-overlay" => Ok(Verb::Hide),
            "toggle" | "toggle-overlay" => Ok(Verb::Toggle),
            "status" => Ok(Verb::Status),
            "reload" => Ok(Verb::Reload),
            "settings" => Ok(Verb::Settings(SettingsPage::Grid)),
            "--list-input-devices" => Ok(Verb::ListInputDevices),
            _ => Err(CliError::Usage),
        }
    }

    /// Parse a `mouseless://` URL back into its verb.
    ///
    /// Only the exact links produced by [`Self::deep_link`] are accepted (a
    /// trailing slash is tolerated); anything else returns `None`.
    pub fn from_deep_link(link: &str) -> Option<Self> {
        Self::from_deep_link_with_scheme(link, "mouseless")
    }

    /// Parse a deep link using a configured URL scheme.
    pub fn from_deep_link_with_scheme(link: &str, scheme: &str) -> Option<Self> {
        let prefix = format!("{scheme}://");
        let path = link.strip_prefix(&prefix)?;
        let path = path.strip_suffix('/').unwrap_or(path);
        if path == "settings" || path.starts_with("settings/") {
            return SettingsPage::parse_with_scheme(link, scheme).map(Verb::Settings);
        }
        match path {
            "show-overlay" => Some(Verb::Show),
            "hide-overlay" => Some(Verb::Hide),
            "toggle-overlay" => Some(Verb::Toggle),
            "status" => Some(Verb::Status),
            "list-input-devices" => Some(Verb::ListInputDevices),
            _ => None,
        }
    }

    /// Deep-link URL for this verb (`mouseless://…`), when one exists.
    ///
    /// `reload` is deliberately socket-only (SPEC-cli): it targets the daemon
    /// process, not a desktop session handler.
    pub fn deep_link(&self) -> Option<&'static str> {
        match self {
            Verb::Show => Some("mouseless://show-overlay"),
            Verb::Hide => Some("mouseless://hide-overlay"),
            Verb::Toggle => Some("mouseless://toggle-overlay"),
            Verb::Status => Some("mouseless://status"),
            Verb::Reload => None,
            Verb::ListInputDevices => Some("mouseless://list-input-devices"),
            Verb::Settings(page) => Some(page.deep_link()),
        }
    }

    /// Deep-link URL using a configured URL scheme.
    pub fn deep_link_with_scheme(&self, scheme: &str) -> Option<String> {
        match self {
            Verb::Reload => None,
            Verb::Settings(page) => Some(page.deep_link_with_scheme(scheme)),
            other => other
                .deep_link()
                .map(|link| format!("{scheme}://{}", link.trim_start_matches("mouseless://"))),
        }
    }

    /// Kanata layer this verb switches to, when the target is unconditional.
    ///
    /// `toggle` needs the current layer first and therefore returns `None`;
    /// see [`toggle_target`]. Layer switches (never direct daemon frames) keep
    /// kanata as the single source of truth (SPEC-cli).
    pub fn layer_change(&self) -> Option<&'static str> {
        self.layer_change_with(&LayerNames::default())
    }

    /// Kanata layer for this verb using configured layer names.
    pub fn layer_change_with<'a>(&self, layers: &LayerNames<'a>) -> Option<&'a str> {
        match self {
            Verb::Show => Some(layers.overlay),
            Verb::Hide => Some(layers.base),
            Verb::Toggle
            | Verb::Status
            | Verb::Reload
            | Verb::ListInputDevices
            | Verb::Settings(_) => None,
        }
    }
}

/// Target layer for `toggle`: hide a visible overlay, otherwise show it.
///
/// Target layer for `toggle`: hide a visible overlay, otherwise show it.
pub fn toggle_target(current_layer: &str) -> &'static str {
    toggle_target_with(current_layer, &LayerNames::default())
}

/// Target layer for `toggle` using configured layer names.
pub fn toggle_target_with<'a>(current_layer: &str, layers: &LayerNames<'a>) -> &'a str {
    if current_layer == layers.overlay {
        layers.base
    } else {
        layers.overlay
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbs_have_deep_link_parity() {
        for (arg, link) in [
            ("show", "mouseless://show-overlay"),
            ("hide", "mouseless://hide-overlay"),
            ("toggle", "mouseless://toggle-overlay"),
            ("status", "mouseless://status"),
            ("--list-input-devices", "mouseless://list-input-devices"),
        ] {
            let verb = Verb::parse(arg).unwrap();
            assert_eq!(verb.deep_link(), Some(link));
            // The scheme handler passes the URL back to the CLI.
            assert_eq!(Verb::parse(link).unwrap(), verb);
            assert_eq!(Verb::from_deep_link(link), Some(verb));
        }
        assert_eq!(Verb::parse("reload").unwrap().deep_link(), None);
        assert_eq!(
            Verb::from_deep_link("mouseless://show-overlay/").map(|v| v.deep_link()),
            Some(Some("mouseless://show-overlay"))
        );
    }

    #[test]
    fn settings_pages_have_deep_link_parity() {
        for page in SettingsPage::ALL {
            let verb = Verb::Settings(page);
            assert_eq!(verb.deep_link(), Some(page.deep_link()));
            assert_eq!(Verb::parse(page.deep_link()).unwrap(), verb);
            assert_eq!(Verb::from_deep_link(page.deep_link()), Some(verb));
            assert_eq!(SettingsPage::parse(page.name()), Some(page));
        }
        // The landing page is also reachable through the short form.
        assert_eq!(
            Verb::parse("mouseless://settings"),
            Ok(Verb::Settings(SettingsPage::Grid))
        );
        assert_eq!(
            Verb::parse("mouseless://settings/grid"),
            Ok(Verb::Settings(SettingsPage::Grid))
        );
        assert_eq!(
            SettingsPage::parse("mouseless://settings/appearance/"),
            Some(SettingsPage::Appearance)
        );
        assert_eq!(
            Verb::parse("settings/appearance"),
            Ok(Verb::Settings(SettingsPage::Appearance))
        );
        assert_eq!(SettingsPage::parse("mouseless://settings/nope"), None);
    }

    #[test]
    fn unknown_verb_prints_usage() {
        assert_eq!(Verb::parse("frobnicate"), Err(CliError::Usage));
    }

    #[test]
    fn unknown_deep_link_is_usage() {
        assert_eq!(Verb::parse("mouseless://"), Err(CliError::Usage));
        assert_eq!(
            Verb::parse("mouseless://show-overlay?x=1"),
            Err(CliError::Usage)
        );
        assert_eq!(
            Verb::parse("mouseless://settings-other"),
            Err(CliError::Usage)
        );
        assert_eq!(Verb::from_deep_link("https://show-overlay"), None);
    }

    #[test]
    fn verbs_map_to_kanata_layers() {
        assert_eq!(Verb::Show.layer_change(), Some(OVERLAY_LAYER));
        assert_eq!(Verb::Hide.layer_change(), Some(BASE_LAYER));
        assert_eq!(Verb::Toggle.layer_change(), None);
        assert_eq!(Verb::Status.layer_change(), None);
        assert_eq!(Verb::Reload.layer_change(), None);
        assert_eq!(Verb::ListInputDevices.layer_change(), None);
        assert_eq!(
            Verb::Settings(SettingsPage::Appearance).layer_change(),
            None
        );
    }

    #[test]
    fn deep_links_accept_a_configured_scheme() {
        assert_eq!(
            Verb::parse_with_scheme("ml://toggle-overlay", "ml"),
            Ok(Verb::Toggle)
        );
        assert_eq!(
            Verb::parse_with_scheme("mouseless://toggle-overlay", "ml"),
            Err(CliError::Usage)
        );
        assert_eq!(
            SettingsPage::parse_with_scheme("ml://settings/appearance", "ml"),
            Some(SettingsPage::Appearance)
        );
        assert_eq!(
            Verb::deep_link_with_scheme(&Verb::Show, "ml"),
            Some("ml://show-overlay".into())
        );
        assert_eq!(Verb::deep_link_with_scheme(&Verb::Reload, "ml"), None);
    }

    #[test]
    fn layer_names_can_come_from_application_config() {
        let layers = LayerNames {
            base: "base-x",
            overlay: "overlay-x",
        };
        assert_eq!(Verb::Hide.layer_change_with(&layers), Some("base-x"));
        assert_eq!(Verb::Show.layer_change_with(&layers), Some("overlay-x"));
        assert_eq!(toggle_target_with("overlay-x", &layers), "base-x");
        assert_eq!(toggle_target_with("base-x", &layers), "overlay-x");
    }

    #[test]
    fn toggle_target_hides_only_a_visible_overlay() {
        assert_eq!(toggle_target(OVERLAY_LAYER), BASE_LAYER);
        assert_eq!(toggle_target(BASE_LAYER), OVERLAY_LAYER);
        assert_eq!(toggle_target("lyr-mouse"), OVERLAY_LAYER);
    }
}
