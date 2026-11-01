//! Persistent settings (JSON under Application Support).

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Ollama,
    OpenAI,
    Anthropic,
    /// publik API: zero setup, key minted per computer (see publik.rs).
    Publik,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmConfig {
    pub provider: Provider,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

impl Default for LlmConfig {
    fn default() -> Self {
        // Local AI first: a model running in Ollama on this computer. The model is
        // picked in Settings → AI; until then the AI features say how to set it up.
        LlmConfig {
            provider: Provider::Ollama,
            base_url: OLLAMA_BASE_URL.into(),
            api_key: String::new(),
            model: String::new(),
        }
    }
}

pub const OLLAMA_BASE_URL: &str = "http://127.0.0.1:11434";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Voice {
    pub id: String,
    pub name: String,
    pub description: String,
    pub custom: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Audience {
    pub id: String,
    pub name: String,
    pub description: String,
    pub custom: bool,
}

/// Plagiarism search. `brave_key` is the user's own Brave Search API key, optional:
/// empty means the check searches through publik API when it is on, else through
/// the keyless chain (features/plagiarism.rs `search_route`). Older files also
/// carried a `provider` field; serde drops it.
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SearchConfig {
    pub brave_key: String,
}

/// What the app remembers about its publik API set-up (the key itself lives in
/// the convention file, see publik.rs).
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PublikSettings {
    /// Set once the disclosure was accepted (0 = never).
    pub disclosure_version: u32,
    /// The first-run card is still owed (CONTRACT §12.4): shown until one of its buttons is tapped.
    pub first_run_owed: bool,
}

/// An app the user switched GSpot off in. Terminals are skipped without
/// being listed here (`host::BUILT_IN_SKIP`).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
pub struct SkipApp {
    pub bundle_id: String,
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub llm: LlmConfig,
    pub dock_side: String, // "left" | "right"
    pub rail_side: String, // "right" | "left"
    pub user_name: String,
    pub search: SearchConfig,
    pub custom_voices: Vec<Voice>,
    pub custom_audiences: Vec<Audience>,
    pub dismissed: HashSet<u64>,
    pub target_language: String,
    pub citation_style: String,
    pub show_gbutton: bool,
    pub show_underlines: bool,
    pub onboarded: bool,
    /// Run the sentence spot check by itself when typing pauses.
    pub auto_spot_check: bool,
    pub publik: PublikSettings,
    /// Apps GSpot stays out of (added from the Review view's "Skip this app").
    pub skip_apps: Vec<SkipApp>,
    /// When the cloud AI hits its limit or cannot be reached, retry on the local model.
    pub ai_fallback_local: bool,
    /// Launch GSpot when the user logs in.
    pub start_with_windows: bool,
    /// Words the user added: never flagged as misspelled (compared case-insensitively).
    pub user_words: Vec<String>,
    /// Built-in skipped apps the user turned GSpot back on in.
    pub unskip_apps: Vec<String>, // bundle ids / exe names
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            llm: LlmConfig::default(),
            dock_side: "left".into(),
            rail_side: "right".into(),
            user_name: default_user_name(),
            search: SearchConfig::default(),
            custom_voices: vec![],
            custom_audiences: vec![],
            dismissed: HashSet::new(),
            target_language: "English (American)".into(),
            citation_style: "APA".into(),
            show_gbutton: true,
            show_underlines: true,
            onboarded: false,
            auto_spot_check: true,
            publik: PublikSettings::default(),
            skip_apps: vec![],
            ai_fallback_local: true,
            start_with_windows: true,
            user_words: vec![],
            unskip_apps: vec![],
        }
    }
}

/// The login name: `USER` on macOS, `USERNAME` on Windows.
fn login_name() -> Result<String, std::env::VarError> {
    std::env::var("USER").or_else(|_| std::env::var("USERNAME"))
}

fn default_user_name() -> String {
    // macOS keeps the account's full name ("Mann Bellani"); `id -F` prints it.
    #[cfg(target_os = "macos")]
    if let Ok(out) = std::process::Command::new("id").arg("-F").output() {
        let full = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if full.split_whitespace().count() >= 1 && full.len() <= 60 && !full.is_empty() {
            return full;
        }
    }
    login_name().map(|u| {
        let mut c = u.chars();
        match c.next() {
            Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            None => "Writer".into(),
        }
    }).unwrap_or_else(|_| "Writer".into())
}

pub fn initials(name: &str) -> String {
    let parts: Vec<&str> = name.split_whitespace().collect();
    let mut s = String::new();
    for p in parts.iter().take(2) {
        if let Some(c) = p.chars().next() {
            s.push(c.to_ascii_uppercase());
        }
    }
    if s.is_empty() {
        "G".into()
    } else {
        s
    }
}

/// Is `original` (a flagged word) one the user added? Trims, drops a trailing
/// possessive "'s" / "’s", compares case-insensitively.
pub fn is_user_word(words: &[String], original: &str) -> bool {
    let w = original.trim();
    let w = w.strip_suffix("'s").or_else(|| w.strip_suffix("’s")).unwrap_or(w).to_lowercase();
    !w.is_empty() && words.iter().any(|u| u.trim().to_lowercase() == w)
}

pub fn support_dir() -> PathBuf {
    let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("/tmp"));
    let dir = base.join("com.publik.grammarlama");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

pub fn settings_path() -> PathBuf {
    support_dir().join("settings.json")
}

pub fn load() -> Settings {
    let mut s = match std::fs::read_to_string(settings_path()) {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => Settings::default(),
    };
    migrate_user_name(&mut s);
    s
}

/// Older builds saved the capitalized login name ("Mannbellani") because they
/// never asked the account for its full name. Replace that stale value with
/// the full name so the avatar initials match the real account ("MB").
fn migrate_user_name(s: &mut Settings) {
    let login_fallback = login_name().map(|u| {
        let mut c = u.chars();
        match c.next() {
            Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            None => String::new(),
        }
    }).unwrap_or_default();
    let stale = s.user_name.trim().is_empty()
        || (!login_fallback.is_empty() && s.user_name == login_fallback);
    if stale {
        s.user_name = default_user_name();
    }
}

pub fn save(s: &Settings) -> anyhow::Result<()> {
    let json = serde_json::to_string_pretty(s)?;
    std::fs::write(settings_path(), json)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_take_first_two_words() {
        assert_eq!(initials("Mann Bellani"), "MB");
        assert_eq!(initials("mann"), "M");
        assert_eq!(initials(""), "G");
        assert_eq!(initials("a b c"), "AB");
    }

    #[test]
    fn migrate_replaces_stale_login_name_but_keeps_custom_name() {
        let login = login_name().unwrap_or_default();
        let mut c = login.chars();
        let stale = match c.next() {
            Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            None => String::new(),
        };
        let mut s = Settings::default();
        s.user_name = stale;
        migrate_user_name(&mut s);
        assert_eq!(s.user_name, default_user_name());

        let mut custom = Settings::default();
        custom.user_name = "Alice Example".into();
        migrate_user_name(&mut custom);
        assert_eq!(custom.user_name, "Alice Example");

        let mut empty = Settings::default();
        empty.user_name = "  ".into();
        migrate_user_name(&mut empty);
        assert!(!empty.user_name.trim().is_empty());
    }

    #[test]
    fn older_settings_files_load_with_defaults_for_new_fields() {
        let old = r#"{"llm":{"provider":"ollama","base_url":"http://127.0.0.1:11434","api_key":"","model":"qwen2.5:7b"},"dock_side":"right","rail_side":"right","user_name":"Alice Example","search":{"provider":"duckduckgo","brave_key":""},"custom_voices":[],"custom_audiences":[],"dismissed":[7],"target_language":"French","citation_style":"MLA","show_gbutton":true,"show_underlines":true,"onboarded":true}"#;
        let s: Settings = serde_json::from_str(old).expect("old file loads");
        assert_eq!(s.user_name, "Alice Example");
        assert_eq!(s.dock_side, "right");
        assert!(s.dismissed.contains(&7));
        assert!(s.auto_spot_check, "new field takes its default");
        assert_eq!(s.publik.disclosure_version, 0);
        let p: Provider = serde_json::from_str("\"publik\"").unwrap();
        assert_eq!(p, Provider::Publik);
    }

    #[test]
    fn local_ai_is_the_default_and_a_saved_local_model_is_kept() {
        let s = Settings::default();
        assert_eq!(s.llm.provider, Provider::Ollama, "local model by default");
        assert_eq!(s.llm.base_url, OLLAMA_BASE_URL);
        let saved = r#"{"llm":{"provider":"ollama","base_url":"http://127.0.0.1:11434","api_key":"","model":"qwen2.5:7b"}}"#;
        let back: Settings = serde_json::from_str(saved).unwrap();
        assert_eq!(back.llm.provider, Provider::Ollama);
        assert_eq!(back.llm.model, "qwen2.5:7b");
    }

    #[test]
    fn fallback_defaults_on_for_old_files() {
        let back: Settings = serde_json::from_str("{}").unwrap();
        assert!(back.ai_fallback_local);
    }

    #[test]
    fn start_with_windows_defaults_on_for_old_files() {
        let back: Settings = serde_json::from_str(r#"{"onboarded":true}"#).unwrap();
        assert!(back.start_with_windows);
    }

    #[test]
    fn settings_round_trip_through_json() {
        let mut s = Settings::default();
        s.dismissed.insert(42);
        s.llm.model = "x".into();
        let j = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&j).unwrap();
        assert!(back.dismissed.contains(&42));
        assert_eq!(back.llm.model, "x");
        assert_eq!(back.dock_side, "left");
    }

    #[test]
    fn old_files_have_no_user_words_or_unskipped_apps() {
        let back: Settings = serde_json::from_str(r#"{"onboarded":true}"#).unwrap();
        assert!(back.user_words.is_empty());
        assert!(back.unskip_apps.is_empty());
    }

    #[test]
    fn user_words_match_case_insensitively_and_ignore_possessives() {
        let words = vec!["Sam".to_string()];
        assert!(is_user_word(&words, "Sam"));
        assert!(is_user_word(&words, "sam's"));
        assert!(is_user_word(&words, "SAM"));
        assert!(is_user_word(&words, " Sam’s "));
        assert!(!is_user_word(&words, "Samuel"));
        assert!(!is_user_word(&[], "Sam"));
    }
}
