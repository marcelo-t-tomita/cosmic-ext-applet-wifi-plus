//! Panel preferences, persisted through cosmic-config so they survive a panel
//! restart and land in ~/.config/cosmic/<APP_ID>/v1/ like every other COSMIC
//! setting.

use cosmic::cosmic_config::{Config, ConfigGet, ConfigSet};

pub const CONFIG_VERSION: u64 = 1;
const SHOW_SSID: &str = "show_ssid";

pub struct Settings {
    /// `None` when cosmic-config is unavailable: the panel still runs, the
    /// preference just does not persist.
    config: Option<Config>,
    pub show_ssid: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            config: None,
            show_ssid: true,
        }
    }
}

impl Settings {
    pub fn load() -> Self {
        let config = Config::new(crate::app::APP_ID, CONFIG_VERSION).ok();
        let show_ssid = config
            .as_ref()
            .and_then(|config| config.get::<bool>(SHOW_SSID).ok())
            .unwrap_or(true);

        Self { config, show_ssid }
    }

    pub fn set_show_ssid(&mut self, value: bool) {
        self.show_ssid = value;

        if let Some(config) = &self.config {
            if let Err(error) = config.set(SHOW_SSID, value) {
                tracing::warn!("could not persist {SHOW_SSID}: {error}");
            }
        }
    }
}
