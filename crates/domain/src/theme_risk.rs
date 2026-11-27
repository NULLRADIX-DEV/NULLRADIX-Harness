//! What a theme package can do beyond styling, and what an update adds to it.

use crate::theme::{ManifestRecord, extension};

/// Facts about a package that a person confirms before it is installed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ThemeRisk {
    /// The package holds script files (`js` or `mjs`).
    pub has_code: bool,
    /// Domains the theme declares in `permissions.network`, lowercase and without repeats.
    pub network: Vec<String>,
    /// The package has a Claude Code theme.
    pub claude_theme: bool,
}

/// What an update brings that the installed version did not have.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RiskAdded {
    pub code: bool,
    pub domains: Vec<String>,
}

impl RiskAdded {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        !self.code && self.domains.is_empty()
    }
}

impl ThemeRisk {
    /// Code or declared domains.
    #[must_use]
    pub fn is_risky(&self) -> bool {
        self.has_code || !self.network.is_empty()
    }

    /// What this package adds to `installed`, or everything risky when nothing is installed.
    #[must_use]
    pub fn added_to(&self, installed: Option<&Self>) -> RiskAdded {
        let known = |domain: &String| installed.is_some_and(|old| old.network.contains(domain));
        RiskAdded {
            code: self.has_code && !installed.is_some_and(|old| old.has_code),
            domains: self
                .network
                .iter()
                .filter(|domain| !known(domain))
                .cloned()
                .collect(),
        }
    }
}

/// Inspects a package by its manifest and the paths of its files.
#[must_use]
pub fn inspect_package<'a>(
    manifest: &ManifestRecord,
    paths: impl IntoIterator<Item = &'a str>,
) -> ThemeRisk {
    let has_code = paths
        .into_iter()
        .any(|path| extension(path).is_some_and(|ext| ext == "js" || ext == "mjs"));
    let mut network: Vec<String> = Vec::new();
    for domain in &manifest.network {
        let domain = domain.to_ascii_lowercase();
        if !network.contains(&domain) {
            network.push(domain);
        }
    }
    ThemeRisk {
        has_code,
        network,
        claude_theme: manifest.claude_theme,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{ManifestDraft, ThemeManifest};

    fn manifest(network: &[&str], claude: bool) -> ManifestRecord {
        let draft = ManifestDraft {
            id: Some("night-blue".into()),
            name: Some("Night Blue".into()),
            version: Some("1.0.0".into()),
            author: Some("Ada".into()),
            sdk_version: Some("^1.3".into()),
            color_scheme: Some("dark".into()),
            network: network
                .iter()
                .enumerate()
                .map(|(index, domain)| (index, (*domain).to_owned()))
                .collect(),
            claude_theme: claude,
            ..ManifestDraft::default()
        };
        ThemeManifest::build(draft, Vec::new())
            .unwrap()
            .record()
            .clone()
    }

    #[test]
    fn a_style_only_package_has_no_risk() {
        let risk = inspect_package(&manifest(&[], false), ["manifest.json", "theme.css"]);
        assert_eq!(risk, ThemeRisk::default());
        assert!(!risk.is_risky());
    }

    #[test]
    fn script_files_anywhere_count_as_code() {
        for path in ["main.js", "lib/util.mjs", "claude/hooks/run.JS"] {
            let risk = inspect_package(&manifest(&[], false), ["theme.css", path]);
            assert!(risk.has_code, "{path}");
        }
        let risk = inspect_package(&manifest(&[], false), ["theme.css", "js/readme.json"]);
        assert!(!risk.has_code);
    }

    #[test]
    fn domains_are_lowercased_and_listed_once() {
        let risk = inspect_package(
            &manifest(
                &[
                    "Fonts.Example.com",
                    "fonts.example.com",
                    "*.cdn.example.com",
                ],
                true,
            ),
            ["theme.css"],
        );
        assert_eq!(risk.network, ["fonts.example.com", "*.cdn.example.com"]);
        assert!(risk.claude_theme);
        assert!(risk.is_risky());
    }

    #[test]
    fn a_new_install_adds_everything_risky() {
        let risk = ThemeRisk {
            has_code: true,
            network: vec!["a.example.com".into()],
            claude_theme: false,
        };
        let added = risk.added_to(None);
        assert!(added.code);
        assert_eq!(added.domains, ["a.example.com"]);
    }

    #[test]
    fn an_update_adds_only_code_and_domains_the_installed_version_lacked() {
        let installed = ThemeRisk {
            has_code: true,
            network: vec!["a.example.com".into()],
            claude_theme: false,
        };
        let same = installed.clone().added_to(Some(&installed));
        assert!(same.is_empty());
        let next = ThemeRisk {
            has_code: true,
            network: vec!["a.example.com".into(), "b.example.com".into()],
            claude_theme: false,
        };
        let added = next.added_to(Some(&installed));
        assert!(!added.code);
        assert_eq!(added.domains, ["b.example.com"]);
        let gained = ThemeRisk {
            has_code: true,
            ..ThemeRisk::default()
        }
        .added_to(Some(&ThemeRisk::default()));
        assert!(gained.code);
        let dropped = ThemeRisk::default().added_to(Some(&installed));
        assert!(dropped.is_empty());
    }
}
