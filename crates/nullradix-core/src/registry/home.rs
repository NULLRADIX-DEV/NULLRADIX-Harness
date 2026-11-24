//! Benutzerweiter Ordner von NULLRADIX (`~/.nullradix`, überschreibbar mit `NULLRADIX_HOME`). Während der
//! Übergangszeit zählen `HARNESS_HOME` und `~/.harness` noch als Rückfall; die Reihenfolge steht
//! genau so in `packages/core/src/registry/HarnessHome.ts`, damit Node-Server und Rust-CLI
//! denselben Ordner nehmen. Alle Pfade darin stehen an dieser einen Stelle.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Woher der Ordner kommt; `migrate` und die Hinweise der CLI entscheiden daran
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HomeSource {
    /// `NULLRADIX_HOME` war gesetzt
    EnvNullradix,
    /// `HARNESS_HOME` war gesetzt (veraltet)
    EnvHarness,
    /// `~/.nullradix` ist vorhanden
    Nullradix,
    /// Nur `~/.harness` ist vorhanden: Rückfall, solange nicht migriert
    HarnessFallback,
    /// Weder noch: Neuinstallation in `~/.nullradix`
    New,
}

impl HomeSource {
    /// Kennung für Ausgaben und JSON
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EnvNullradix => "env-nullradix",
            Self::EnvHarness => "env-harness",
            Self::Nullradix => "nullradix",
            Self::HarnessFallback => "harness-fallback",
            Self::New => "new",
        }
    }
}

/// Name des Ordners von NULLRADIX im Benutzerordner
pub const NULLRADIX_DIR: &str = ".nullradix";

/// Name des alten Ordners des Harness im Benutzerordner (Rückfall der Übergangszeit)
pub const HARNESS_DIR: &str = ".harness";

/// Name des Ordners unter dem früheren Namen Nexus (Rückfall der Übergangszeit, vor `.harness`)
pub const NEXUS_DIR: &str = ".nexus";

/// Übernimmt Umgebungsvariablen unter dem früheren Namen: ist `NEXUS_X` gesetzt und
/// `NULLRADIX_X` nicht, gilt danach `NULLRADIX_X` mit demselben Wert. Läuft einmal beim Start
/// jedes Programms, bevor Threads entstehen.
pub fn adopt_legacy_env() {
    let legacy: Vec<(String, String)> = std::env::vars()
        .filter_map(|(name, value)| {
            let rest = name.strip_prefix("NEXUS_")?;
            let new = format!("NULLRADIX_{rest}");
            std::env::var_os(&new).is_none().then_some((new, value))
        })
        .collect();
    for (name, value) in legacy {
        std::env::set_var(name, value);
    }
}

/// Benutzerweiter Ordner von NULLRADIX und alle Dateien darin
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NullradixHome {
    dir: PathBuf,
    source: HomeSource,
}

/// Alter Name des Typs; die Aufrufer werden schrittweise umgestellt
pub type HarnessHome = NullradixHome;

impl NullradixHome {
    /// Ordner aus einer Umgebung (siehe [`NullradixHome::from_env`])
    pub fn new(env: &HashMap<String, String>) -> Self {
        Self::from_env(|name| env.get(name).cloned())
    }

    /// Ordner aus der Umgebung des laufenden Prozesses
    pub fn from_process_env() -> Self {
        Self::from_env(|name| std::env::var(name).ok())
    }

    /// Ordner aus einer frei wählbaren Umgebung. Reihenfolge: `NULLRADIX_HOME`, `HARNESS_HOME`
    /// (veraltet), vorhandenes `~/.nullradix`, vorhandenes `~/.harness` (Rückfall), sonst `~/.nullradix`.
    /// Ein leerer Wert zählt wie keiner.
    pub fn from_env(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let value = |name| lookup(name).filter(|value: &String| !value.is_empty());
        if let Some(dir) = value("NULLRADIX_HOME") {
            return Self::with_source(dir, HomeSource::EnvNullradix);
        }
        if let Some(dir) = value("HARNESS_HOME") {
            return Self::with_source(dir, HomeSource::EnvHarness);
        }
        Self::in_home_dir(&home_dir())
    }

    /// Ordner unter einem Benutzerordner: `.nullradix`, sonst `.nexus` oder `.harness` als
    /// Rückfall, sonst `.nullradix` als Neuinstallation
    pub fn in_home_dir(base: &Path) -> Self {
        let nullradix = base.join(NULLRADIX_DIR);
        if nullradix.is_dir() {
            return Self::with_source(nullradix, HomeSource::Nullradix);
        }
        let nexus = base.join(NEXUS_DIR);
        if nexus.is_dir() {
            return Self::with_source(nexus, HomeSource::HarnessFallback);
        }
        let harness = base.join(HARNESS_DIR);
        if harness.is_dir() {
            return Self::with_source(harness, HomeSource::HarnessFallback);
        }
        Self::with_source(nullradix, HomeSource::New)
    }

    /// Ordner an einem bekannten Pfad (Tests und Aufrufer, die ihn schon kennen)
    pub fn at(dir: impl Into<PathBuf>) -> Self {
        Self::with_source(dir, HomeSource::Nullradix)
    }

    /// Ordner an einem bekannten Pfad mit bekannter Herkunft
    pub fn with_source(dir: impl Into<PathBuf>, source: HomeSource) -> Self {
        Self {
            dir: dir.into(),
            source,
        }
    }

    /// Der Ordner selbst
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Woher der Ordner kommt
    pub fn source(&self) -> HomeSource {
        self.source
    }

    /// Liste der angemeldeten Projekte
    pub fn projects_file(&self) -> PathBuf {
        self.dir.join("projects.json")
    }

    /// Sperre der Projektliste
    pub fn projects_lock(&self) -> PathBuf {
        self.dir.join("projects.lock")
    }

    /// Prozessangaben des laufenden Servers
    pub fn server_file(&self) -> PathBuf {
        self.dir.join("server.json")
    }

    /// Protokoll des Servers
    pub fn log_file(&self) -> PathBuf {
        self.dir.join("server.log")
    }

    /// Bekannte Claude-Sessions
    pub fn sessions_file(&self) -> PathBuf {
        self.dir.join("sessions.json")
    }

    /// Favoriten des Benutzers
    pub fn favorites_file(&self) -> PathBuf {
        self.dir.join("favorites.json")
    }

    /// Benutzerweite Einstellungen
    pub fn settings_file(&self) -> PathBuf {
        self.dir.join("settings.json")
    }

    /// Claude-Accounts des Dispatchers
    pub fn accounts_file(&self) -> PathBuf {
        self.dir.join("accounts.json")
    }

    /// Langzeit-Tokens der Accounts für die Läufe (`claude setup-token`); nur für den Benutzer
    pub fn account_tokens_file(&self) -> PathBuf {
        self.dir.join("account-tokens.json")
    }

    /// Konfigurationsordner neu angelegter Accounts (`<id>/`)
    pub fn accounts_dir(&self) -> PathBuf {
        self.dir.join("accounts")
    }

    /// Installierte Theme-Pakete (`<id>/`)
    pub fn themes_dir(&self) -> PathBuf {
        self.dir.join("themes")
    }

    /// Sicherungen
    pub fn backups_dir(&self) -> PathBuf {
        self.dir.join("backups")
    }

    /// Verbindung zur Board-Datenbank
    pub fn board_config_file(&self) -> PathBuf {
        self.dir.join("board.yaml")
    }

    /// Standardpfad der SQLite-Datei des Boards (Anbieter `sqlite` ohne eigenes `file`)
    pub fn board_db_file(&self) -> PathBuf {
        self.dir.join("board.db")
    }

    /// Konfiguration des Dispatchers
    pub fn dispatch_config_file(&self) -> PathBuf {
        self.dir.join("dispatch.yaml")
    }

    /// Einstellung des Telegram-Bots (mit Geheimnissen, deshalb nur für den Benutzer)
    pub fn telegram_config_file(&self) -> PathBuf {
        self.dir.join("telegram.yaml")
    }

    /// Ordner des Telegram-Bots
    pub fn telegram_dir(&self) -> PathBuf {
        self.dir.join("telegram")
    }

    /// Laufzeit-Zustand des Telegram-Bots (abgeholte Updates); keine Geheimnisse
    pub fn telegram_state_file(&self) -> PathBuf {
        self.telegram_dir().join("state.json")
    }

    /// Offene Ideen-Entwürfe des Telegram-Bots je Chat; keine Geheimnisse
    pub fn telegram_drafts_file(&self) -> PathBuf {
        self.telegram_dir().join("drafts.json")
    }

    /// Ausblendungen und letzte Claude-Analyse der Optimierungs-Vorschläge
    pub fn dispatch_suggestions_file(&self) -> PathBuf {
        self.dir.join("dispatch-suggestions.json")
    }

    /// Eigene Repo-Pfade der Boards (je Benutzer lokal, nicht in der gemeinsamen DB)
    pub fn board_repos_file(&self) -> PathBuf {
        self.dir.join("board-repos.yaml")
    }

    /// Eigene Reihenfolge von „Meine Aufgaben“ (je Benutzer lokal)
    pub fn inbox_order_file(&self) -> PathBuf {
        self.dir.join("inbox-order.json")
    }

    /// CSS-Schnipsel des Nutzers (`<id>.css` und `snippets.json`)
    pub fn snippets_dir(&self) -> PathBuf {
        self.dir.join("snippets")
    }

    /// Einstellungen der Themes je Theme-ID (`theme-settings.json`)
    pub fn theme_settings_file(&self) -> PathBuf {
        self.dir.join("theme-settings.json")
    }

    /// Theme-Store des Nutzers (`repos.json`)
    pub fn store_dir(&self) -> PathBuf {
        self.dir.join("store")
    }

    /// Ordner des Dispatchers
    pub fn dispatch_dir(&self) -> PathBuf {
        self.dir.join("dispatch")
    }

    /// Läufe des Dispatchers
    pub fn dispatch_runs_dir(&self) -> PathBuf {
        self.dispatch_dir().join("runs")
    }

    /// Zustand des Dispatchers
    pub fn dispatch_state_file(&self) -> PathBuf {
        self.dispatch_dir().join("state.json")
    }

    /// Verbrauchsproben der Accounts (eine JSON-Zeile je beendetem Lauf)
    pub fn account_usage_file(&self) -> PathBuf {
        self.dispatch_dir().join("account-usage.jsonl")
    }

    /// Review-Einträge vor dem Merge (`<Ticket-ID>.json`)
    pub fn dispatch_reviews_dir(&self) -> PathBuf {
        self.dispatch_dir().join("reviews")
    }

    /// Leerer Arbeitsordner der Claude-Analyse (Optimierungs-Vorschläge)
    pub fn dispatch_analysis_dir(&self) -> PathBuf {
        self.dispatch_dir().join("analysis")
    }

    /// Protokolle der Dispatcher-Läufe
    pub fn dispatch_logs_dir(&self) -> PathBuf {
        self.dir.join("dispatch-logs")
    }

    /// Zentrale Regeln, die die Projekte über ihre Rule importieren (z. B. `docs.md`)
    pub fn rules_dir(&self) -> PathBuf {
        self.dir.join("rules")
    }

    /// Angewendete Anpassungen (Workflow, Übersicht), geschrieben von `nullradix install`
    pub fn customizations_dir(&self) -> PathBuf {
        self.dir.join("customizations")
    }

    /// Lokaler Marktplatz mit der Kopie des mitgelieferten Plugins (`plugin/nullradix`); überlebt
    /// Updates der App, deren Ressourcenordner das nicht tut
    pub fn plugin_dir(&self) -> PathBuf {
        self.dir.join("plugin")
    }
}

/// Quelle eines Umzugs: `HARNESS_HOME`, sonst ein vorhandenes `~/.nexus`, sonst `~/.harness` –
/// beim letzten unabhängig davon, ob es den Ordner gibt
pub fn harness_dir(lookup: impl Fn(&str) -> Option<String>) -> PathBuf {
    match lookup("HARNESS_HOME").filter(|value| !value.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => {
            let nexus = home_dir().join(NEXUS_DIR);
            if nexus.is_dir() {
                nexus
            } else {
                home_dir().join(HARNESS_DIR)
            }
        }
    }
}

/// Ziel eines Umzugs: `NULLRADIX_HOME`, sonst `~/.nullradix` – unabhängig davon, ob es den Ordner gibt
pub fn nullradix_dir(lookup: impl Fn(&str) -> Option<String>) -> PathBuf {
    match lookup("NULLRADIX_HOME").filter(|value| !value.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => home_dir().join(NULLRADIX_DIR),
    }
}

/// Die Umgebung dieses Prozesses als Suche für [`harness_dir`] und [`nullradix_dir`]
pub fn process_lookup(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

/// Benutzerordner des angemeldeten Benutzers, wie `os.homedir()` in Node
fn home_dir() -> PathBuf {
    #[allow(deprecated)]
    std::env::home_dir().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    /// Umgebung mit frei wählbaren Variablen
    fn env(paare: &[(&str, &str)]) -> HashMap<String, String> {
        paare
            .iter()
            .map(|(name, wert)| (name.to_string(), wert.to_string()))
            .collect()
    }

    #[test]
    fn nullradix_home_hat_vorrang_vor_harness_home() {
        let home = NullradixHome::new(&env(&[
            ("NULLRADIX_HOME", "/x/n"),
            ("HARNESS_HOME", "/x/h"),
        ]));
        assert_eq!(home.dir(), Path::new("/x/n"));
        assert_eq!(home.source(), HomeSource::EnvNullradix);
    }

    #[test]
    fn themes_dir_liegt_unter_nullradix_home() {
        let home = NullradixHome::new(&env(&[("NULLRADIX_HOME", "/x/n")]));
        assert_eq!(home.themes_dir(), Path::new("/x/n").join("themes"));
    }

    #[test]
    fn ohne_nullradix_home_gilt_harness_home_als_veralteter_rueckfall() {
        let home = NullradixHome::new(&env(&[("HARNESS_HOME", "/x/h")]));
        assert_eq!(home.dir(), Path::new("/x/h"));
        assert_eq!(home.source(), HomeSource::EnvHarness);
    }

    #[test]
    fn leere_werte_zahlen_wie_keine() {
        let home = NullradixHome::new(&env(&[("NULLRADIX_HOME", ""), ("HARNESS_HOME", "/x/h")]));
        assert_eq!(home.dir(), Path::new("/x/h"));

        let leer = NullradixHome::new(&env(&[("NULLRADIX_HOME", ""), ("HARNESS_HOME", "")]));
        assert_eq!(
            leer.source(),
            NullradixHome::in_home_dir(&home_dir()).source()
        );
    }

    #[test]
    fn ohne_variablen_gilt_punkt_nullradix_im_benutzerordner() {
        let base = tempfile::tempdir().unwrap();
        fs::create_dir(base.path().join(".nullradix")).unwrap();
        fs::create_dir(base.path().join(".harness")).unwrap();

        let home = NullradixHome::in_home_dir(base.path());

        assert_eq!(home.dir(), base.path().join(".nullradix"));
        assert_eq!(home.source(), HomeSource::Nullradix);
    }

    #[test]
    fn ohne_punkt_nullradix_fallt_es_auf_punkt_harness_zurueck() {
        let base = tempfile::tempdir().unwrap();
        fs::create_dir(base.path().join(".harness")).unwrap();

        let home = NullradixHome::in_home_dir(base.path());

        assert_eq!(home.dir(), base.path().join(".harness"));
        assert_eq!(home.source(), HomeSource::HarnessFallback);
    }

    #[test]
    fn ohne_beide_ordner_ist_es_eine_neuinstallation_in_punkt_nullradix() {
        let base = tempfile::tempdir().unwrap();

        let home = NullradixHome::in_home_dir(base.path());

        assert_eq!(home.dir(), base.path().join(".nullradix"));
        assert_eq!(home.source(), HomeSource::New);
    }

    #[test]
    fn quelle_und_ziel_eines_umzugs_kommen_aus_den_variablen() {
        let env = env(&[("NULLRADIX_HOME", "/x/n"), ("HARNESS_HOME", "/x/h")]);
        let lookup = |name: &str| env.get(name).cloned();
        assert_eq!(harness_dir(lookup), Path::new("/x/h"));
        assert_eq!(nullradix_dir(lookup), Path::new("/x/n"));
    }

    #[test]
    fn ohne_variablen_liegen_quelle_und_ziel_im_benutzerordner() {
        let leer = |_: &str| None;
        // Ein vorhandenes ~/.nexus geht als Quelle vor ~/.harness
        let nexus = home_dir().join(NEXUS_DIR);
        let quelle = if nexus.is_dir() {
            nexus
        } else {
            home_dir().join(HARNESS_DIR)
        };
        assert_eq!(harness_dir(leer), quelle);
        assert_eq!(nullradix_dir(leer), home_dir().join(NULLRADIX_DIR));
    }

    #[test]
    fn die_herkunft_hat_eine_kennung_fuer_ausgaben() {
        assert_eq!(HomeSource::EnvNullradix.as_str(), "env-nullradix");
        assert_eq!(HomeSource::EnvHarness.as_str(), "env-harness");
        assert_eq!(HomeSource::Nullradix.as_str(), "nullradix");
        assert_eq!(HomeSource::HarnessFallback.as_str(), "harness-fallback");
        assert_eq!(HomeSource::New.as_str(), "new");
    }

    #[test]
    fn nennt_den_ordner_der_angewendeten_anpassungen() {
        let home = HarnessHome::at(Path::new("x").join("h"));
        assert_eq!(
            home.customizations_dir(),
            Path::new("x").join("h").join("customizations")
        );
    }

    #[test]
    fn nennt_die_standarddatei_der_board_datenbank() {
        let home = HarnessHome::at(Path::new("x").join("h"));
        assert_eq!(
            home.board_db_file(),
            Path::new("x").join("h").join("board.db")
        );
    }

    #[test]
    fn kennt_alle_dateien_und_ordner_des_benutzerordners() {
        let home = HarnessHome::at("h");
        assert_eq!(home.projects_file(), Path::new("h").join("projects.json"));
        assert_eq!(home.projects_lock(), Path::new("h").join("projects.lock"));
        assert_eq!(home.server_file(), Path::new("h").join("server.json"));
        assert_eq!(home.log_file(), Path::new("h").join("server.log"));
        assert_eq!(home.sessions_file(), Path::new("h").join("sessions.json"));
        assert_eq!(home.favorites_file(), Path::new("h").join("favorites.json"));
        assert_eq!(home.settings_file(), Path::new("h").join("settings.json"));
        assert_eq!(
            home.theme_settings_file(),
            Path::new("h").join("theme-settings.json")
        );
        assert_eq!(home.accounts_file(), Path::new("h").join("accounts.json"));
        assert_eq!(home.accounts_dir(), Path::new("h").join("accounts"));
        assert_eq!(home.backups_dir(), Path::new("h").join("backups"));
        assert_eq!(home.board_config_file(), Path::new("h").join("board.yaml"));
        assert_eq!(
            home.dispatch_config_file(),
            Path::new("h").join("dispatch.yaml")
        );
        assert_eq!(
            home.telegram_config_file(),
            Path::new("h").join("telegram.yaml")
        );
        assert_eq!(home.telegram_dir(), Path::new("h").join("telegram"));
        assert_eq!(
            home.telegram_state_file(),
            Path::new("h").join("telegram").join("state.json")
        );
        assert_eq!(
            home.dispatch_suggestions_file(),
            Path::new("h").join("dispatch-suggestions.json")
        );
        assert_eq!(
            home.board_repos_file(),
            Path::new("h").join("board-repos.yaml")
        );
        assert_eq!(
            home.inbox_order_file(),
            Path::new("h").join("inbox-order.json")
        );
        assert_eq!(home.dispatch_dir(), Path::new("h").join("dispatch"));
        assert_eq!(
            home.dispatch_runs_dir(),
            Path::new("h").join("dispatch").join("runs")
        );
        assert_eq!(
            home.dispatch_state_file(),
            Path::new("h").join("dispatch").join("state.json")
        );
        assert_eq!(
            home.dispatch_reviews_dir(),
            Path::new("h").join("dispatch").join("reviews")
        );
        assert_eq!(
            home.dispatch_analysis_dir(),
            Path::new("h").join("dispatch").join("analysis")
        );
        assert_eq!(
            home.dispatch_logs_dir(),
            Path::new("h").join("dispatch-logs")
        );
        assert_eq!(home.rules_dir(), Path::new("h").join("rules"));
    }
}
