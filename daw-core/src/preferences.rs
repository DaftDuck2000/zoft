use crate::project::{Preferences, AudioPreferences, UiPreferences, EditingPreferences};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreferencesManager {
    preferences: Preferences,
    config_path: Option<PathBuf>,
}

impl PreferencesManager {
    pub fn new() -> Self {
        let config_path = Self::default_config_path();
        let preferences = if config_path.exists() {
            Self::load_from_file(&config_path).unwrap_or_default()
        } else {
            Preferences::default()
        };

        Self {
            preferences,
            config_path: Some(config_path),
        }
    }

    pub fn preferences(&self) -> &Preferences {
        &self.preferences
    }

    pub fn preferences_mut(&mut self) -> &mut Preferences {
        &mut self.preferences
    }

    pub fn save(&self) -> Result<()> {
        if let Some(path) = &self.config_path {
            Self::save_to_file(&self.preferences, path)?;
        }
        Ok(())
    }

    fn default_config_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("zoft")
            .join("preferences.toml")
    }

    fn load_from_file(path: &PathBuf) -> Result<Preferences> {
        let content = std::fs::read_to_string(path)?;
        let prefs: Preferences = toml::from_str(&content)?;
        Ok(prefs)
    }

    fn save_to_file(prefs: &Preferences, path: &PathBuf) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(prefs)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    pub fn reset_to_defaults(&mut self) {
        self.preferences = Preferences::default();
    }

    pub fn audio_prefs(&self) -> &AudioPreferences {
        &self.preferences.audio
    }

    pub fn audio_prefs_mut(&mut self) -> &mut AudioPreferences {
        &mut self.preferences.audio
    }

    pub fn ui_prefs(&self) -> &UiPreferences {
        &self.preferences.ui
    }

    pub fn ui_prefs_mut(&mut self) -> &mut UiPreferences {
        &mut self.preferences.ui
    }

    pub fn editing_prefs(&self) -> &EditingPreferences {
        &self.preferences.editing
    }

    pub fn editing_prefs_mut(&mut self) -> &mut EditingPreferences {
        &mut self.preferences.editing
    }
}

impl Default for PreferencesManager {
    fn default() -> Self {
        Self::new()
    }
}