//! Installed themes: the manifest rules and the rules for the files of a package.

use std::collections::{BTreeMap, HashSet};
use std::fmt;

use strum::{EnumString, IntoStaticStr};

use crate::{Timestamp, Version};

/// Themes the web UI ships; the first one is the default.
pub const BUILTIN_THEMES: [&str; 4] = ["nullradix", "system", "light", "dark"];
pub const DEFAULT_THEME: &str = "nullradix";
const THEME_ID_MIN: usize = 2;
pub const THEME_ID_MAX: usize = 40;
pub(crate) const SETTING_KEY_MAX: usize = 40;
pub(crate) const SETTINGS_MAX: usize = 50;
/// Largest package and largest sum of its unpacked files.
pub const MAX_PACKAGE_BYTES: u64 = 20 * 1024 * 1024;
pub const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
pub const MAX_ENTRIES: usize = 500;
pub const MAX_ENTRY_PATH: usize = 512;
pub const MANIFEST_FILE: &str = "manifest.json";
pub const STYLE_FILE: &str = "theme.css";
/// Folder of the Claude Code theme inside a package.
pub const CLAUDE_DIR: &str = "claude";
pub const CLAUDE_HOOKS_FILE: &str = "claude/hooks/hooks.json";

const ALLOWED_EXTENSIONS: &[&str] = &[
    "css", "js", "mjs", "json", "png", "jpg", "jpeg", "gif", "webp", "svg", "avif", "ico", "woff",
    "woff2", "ttf", "otf", "mp3", "ogg", "wav",
];
/// Allowed in addition below `claude/`.
const CLAUDE_EXTENSIONS: &[&str] = &["ts", "tsx"];
/// SDK versions from before the Claude Code theme; a theme with one must not match them.
const SDK_BEFORE_CLAUDE_THEME: [(u64, u64, u64); 3] = [(1, 0, 0), (1, 1, 0), (1, 2, 0)];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ThemeError {
    #[error("a theme id has {THEME_ID_MIN} to {THEME_ID_MAX} characters from a-z, 0-9 and -")]
    InvalidId,
    #[error("the theme {0} is not installed")]
    NotInstalled(String),
}

/// Identifier of a theme: the folder name below the themes directory.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ThemeId(String);

impl ThemeId {
    pub fn parse(text: &str) -> Result<Self, ThemeError> {
        let valid = (THEME_ID_MIN..=THEME_ID_MAX).contains(&text.len())
            && text
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
        valid
            .then(|| Self(text.to_owned()))
            .ok_or(ThemeError::InvalidId)
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether the web UI ships this theme itself.
    #[must_use]
    pub fn is_builtin(&self) -> bool {
        BUILTIN_THEMES.contains(&self.0.as_str())
    }
}

impl fmt::Display for ThemeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, IntoStaticStr, EnumString)]
#[strum(serialize_all = "kebab-case")]
pub enum ColorScheme {
    Light,
    Dark,
}

impl ColorScheme {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        self.into()
    }
}

/// One refused field of a manifest; lists carry the index, such as `slots[1]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldError {
    pub field: String,
    pub message: String,
}

impl FieldError {
    #[must_use]
    pub fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectOption {
    pub value: String,
    pub label: String,
}

/// A setting the theme offers: a switch or a choice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingSpec {
    Boolean {
        label: String,
        hint: Option<String>,
        default: bool,
    },
    Select {
        label: String,
        hint: Option<String>,
        options: Vec<SelectOption>,
        default: String,
    },
}

impl SettingSpec {
    fn label(&self) -> &str {
        match self {
            Self::Boolean { label, .. } | Self::Select { label, .. } => label,
        }
    }
}

/// The fields of a manifest as read from its file, before any rule is applied.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ManifestDraft {
    pub id: Option<String>,
    pub name: Option<String>,
    pub version: Option<String>,
    pub author: Option<String>,
    pub sdk_version: Option<String>,
    pub color_scheme: Option<String>,
    pub start_color: Option<String>,
    pub preview: Option<String>,
    /// Slot names with their position in the list.
    pub slots: Vec<(usize, String)>,
    /// Domains the theme may load from, with their position.
    pub network: Vec<(usize, String)>,
    pub settings: Option<BTreeMap<String, SettingSpec>>,
    pub claude_theme: bool,
}

/// Every field of a valid manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestRecord {
    pub id: ThemeId,
    pub name: String,
    pub version: String,
    pub author: String,
    pub sdk_version: String,
    pub color_scheme: ColorScheme,
    pub start_color: Option<String>,
    pub preview: Option<String>,
    pub slots: Vec<String>,
    pub network: Vec<String>,
    pub settings: Option<BTreeMap<String, SettingSpec>>,
    pub claude_theme: bool,
}

/// The `manifest.json` of a theme package, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeManifest(ManifestRecord);

const FIELD_ORDER: [&str; 12] = [
    "id",
    "name",
    "version",
    "author",
    "sdkVersion",
    "colorScheme",
    "startColor",
    "preview",
    "slots",
    "permissions",
    "settingsSchema",
    "claudeTheme",
];

fn field_rank(field: &str) -> usize {
    let base = field.split(['[', '.']).next().unwrap_or(field);
    FIELD_ORDER
        .iter()
        .position(|name| *name == base)
        .unwrap_or(0)
}

fn missing(field: &str) -> FieldError {
    FieldError::new(field, format!("{field} is missing"))
}

fn blank(text: &str) -> bool {
    text.trim().is_empty()
}

/// Longest name or author, as the store allows.
const MAX_TEXT: usize = 1000;
/// Longest theme or sdk version text, as the store allows.
const MAX_VERSION: usize = 100;
const PREVIEW_EXTENSIONS: [&str; 5] = ["png", "jpg", "jpeg", "webp", "svg"];

fn check_length(text: Option<&str>, field: &str, max: usize, errors: &mut Vec<FieldError>) {
    if text.is_some_and(|text| text.chars().count() > max) {
        errors.push(FieldError::new(
            field,
            format!("{field} is longer than {max} characters"),
        ));
    }
}

fn check_start_color(color: Option<&str>, errors: &mut Vec<FieldError>) {
    let Some(color) = color else { return };
    let digits = color.strip_prefix('#').unwrap_or_default();
    if !(3..=8).contains(&digits.len()) || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        errors.push(FieldError::new(
            "startColor",
            "startColor is a hex color such as #0c0c0e",
        ));
    }
}

fn check_preview(preview: Option<&str>, errors: &mut Vec<FieldError>) {
    let Some(preview) = preview else { return };
    let image = extension(preview).is_some_and(|ext| PREVIEW_EXTENSIONS.contains(&ext.as_str()));
    if preview.ends_with('/') || check_entry_path(preview).is_err() || !image {
        errors.push(FieldError::new(
            "preview",
            "preview is a relative path to a png, jpg, webp or svg file",
        ));
    }
}

impl ThemeManifest {
    /// Checks every field and reports all problems together, `earlier` ones first sorted in.
    pub fn build(draft: ManifestDraft, earlier: Vec<FieldError>) -> Result<Self, Vec<FieldError>> {
        let mut errors = earlier;
        let id = required(draft.id.as_deref(), "id", &mut errors).and_then(|text| {
            ThemeId::parse(text)
                .map_err(|error| errors.push(FieldError::new("id", error.to_string())))
                .ok()
        });
        let name = required(draft.name.as_deref(), "name", &mut errors);
        if name.is_some_and(blank) {
            errors.push(FieldError::new("name", "the name is empty"));
        }
        let version = required(draft.version.as_deref(), "version", &mut errors);
        if version.is_some_and(|text| semver::Version::parse(text).is_err()) {
            errors.push(FieldError::new(
                "version",
                "the version is not a semver version such as 1.2.3",
            ));
        }
        check_length(name, "name", MAX_TEXT, &mut errors);
        check_length(version, "version", MAX_VERSION, &mut errors);
        let author = required(draft.author.as_deref(), "author", &mut errors);
        check_length(author, "author", MAX_TEXT, &mut errors);
        if author.is_some_and(blank) {
            errors.push(FieldError::new("author", "the author is empty"));
        }
        let sdk = required(draft.sdk_version.as_deref(), "sdkVersion", &mut errors);
        check_length(sdk, "sdkVersion", MAX_VERSION, &mut errors);
        check_sdk_range(sdk, draft.claude_theme, &mut errors);
        check_start_color(draft.start_color.as_deref(), &mut errors);
        check_preview(draft.preview.as_deref(), &mut errors);
        let scheme =
            required(draft.color_scheme.as_deref(), "colorScheme", &mut errors).and_then(|text| {
                text.parse::<ColorScheme>()
                    .map_err(|_| {
                        errors.push(FieldError::new(
                            "colorScheme",
                            "colorScheme is light or dark",
                        ));
                    })
                    .ok()
            });
        check_slots(&draft.slots, &mut errors);
        for (index, domain) in &draft.network {
            if let Some(message) = domain_problem(domain) {
                errors.push(FieldError::new(
                    format!("permissions.network[{index}]"),
                    message,
                ));
            }
        }
        if let Some(settings) = &draft.settings {
            check_settings(settings, &mut errors);
        }
        errors.sort_by_key(|error| field_rank(&error.field));
        let (Some(id), Some(scheme), true) = (id, scheme, errors.is_empty()) else {
            return Err(errors);
        };
        let text = |value: &Option<String>| value.clone().unwrap_or_default();
        Ok(Self(ManifestRecord {
            id,
            name: text(&draft.name),
            version: text(&draft.version),
            author: text(&draft.author),
            sdk_version: text(&draft.sdk_version),
            color_scheme: scheme,
            start_color: draft.start_color,
            preview: draft.preview,
            slots: draft.slots.into_iter().map(|(_, slot)| slot).collect(),
            network: draft
                .network
                .into_iter()
                .map(|(_, domain)| domain)
                .collect(),
            settings: draft.settings,
            claude_theme: draft.claude_theme,
        }))
    }

    #[must_use]
    pub const fn record(&self) -> &ManifestRecord {
        &self.0
    }

    #[must_use]
    pub fn id(&self) -> &ThemeId {
        &self.0.id
    }

    /// For stores only: trusts the record as stored.
    #[must_use]
    pub const fn restore(record: ManifestRecord) -> Self {
        Self(record)
    }
}

fn required<'a>(
    value: Option<&'a str>,
    field: &str,
    errors: &mut Vec<FieldError>,
) -> Option<&'a str> {
    if value.is_none() && !errors.iter().any(|error| error.field == field) {
        errors.push(missing(field));
    }
    value
}

/// The range must parse; a theme with a Claude Code theme must not match old SDKs.
fn check_sdk_range(range: Option<&str>, claude_theme: bool, errors: &mut Vec<FieldError>) {
    let Some(range) = range else {
        return;
    };
    let Some(requirement) = (!blank(range))
        .then(|| semver::VersionReq::parse(range).ok())
        .flatten()
    else {
        errors.push(FieldError::new(
            "sdkVersion",
            "sdkVersion is not a semver range such as ^1.0",
        ));
        return;
    };
    let too_old = SDK_BEFORE_CLAUDE_THEME
        .iter()
        .any(|&(major, minor, patch)| {
            requirement.matches(&semver::Version::new(major, minor, patch))
        });
    if claude_theme && too_old {
        errors.push(FieldError::new(
            "sdkVersion",
            "a theme with a Claude Code theme needs sdkVersion 1.3 or newer, such as ^1.3",
        ));
    }
}

fn plain_slot_name(name: &str) -> bool {
    name.starts_with(|letter: char| letter.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|letter| letter.is_ascii_alphanumeric() || matches!(letter, '.' | ':' | '-'))
}

fn check_slots(slots: &[(usize, String)], errors: &mut Vec<FieldError>) {
    let mut seen = HashSet::new();
    for (index, name) in slots {
        let field = format!("slots[{index}]");
        if blank(name) {
            errors.push(FieldError::new(field, "the slot name is empty"));
        } else if !plain_slot_name(name) {
            errors.push(FieldError::new(
                field,
                format!("the slot name {name} may only use letters, digits, . : and -"),
            ));
        } else if !seen.insert(name.as_str()) {
            errors.push(FieldError::new(
                field,
                format!("the slot {name} is listed twice"),
            ));
        }
    }
}

fn valid_setting_key(key: &str) -> bool {
    key.len() <= SETTING_KEY_MAX
        && key.starts_with(|letter: char| letter.is_ascii_alphabetic())
        && key
            .chars()
            .all(|letter| letter.is_ascii_alphanumeric() || matches!(letter, '_' | '-'))
}

fn check_settings(settings: &BTreeMap<String, SettingSpec>, errors: &mut Vec<FieldError>) {
    if settings.len() > SETTINGS_MAX {
        errors.push(FieldError::new(
            "settingsSchema",
            format!("a theme has at most {SETTINGS_MAX} settings"),
        ));
    }
    for (key, spec) in settings {
        let field = format!("settingsSchema.{key}");
        if !valid_setting_key(key) {
            errors.push(FieldError::new(
                &field,
                format!("the key {key} starts with a letter and uses a-z, A-Z, 0-9, _ and - (at most {SETTING_KEY_MAX})"),
            ));
        }
        if blank(spec.label()) {
            errors.push(FieldError::new(
                format!("{field}.label"),
                "the label is empty",
            ));
        }
        let SettingSpec::Select {
            options, default, ..
        } = spec
        else {
            continue;
        };
        if options.is_empty() {
            errors.push(FieldError::new(
                format!("{field}.options"),
                "a choice needs at least one option",
            ));
            continue;
        }
        let mut seen = HashSet::new();
        for (index, option) in options.iter().enumerate() {
            if !seen.insert(option.value.as_str()) {
                errors.push(FieldError::new(
                    format!("{field}.options[{index}]"),
                    format!("the option {} is listed twice", option.value),
                ));
            }
        }
        if !seen.contains(default.as_str()) {
            errors.push(FieldError::new(
                format!("{field}.default"),
                format!("the default {default} is none of the options"),
            ));
        }
    }
}

fn domain_shape(domain: &str) -> bool {
    let name = domain.strip_prefix("*.").unwrap_or(domain);
    let labels: Vec<&str> = name.split('.').collect();
    labels.len() >= 2
        && labels.iter().all(|label| {
            !label.is_empty()
                && label
                    .chars()
                    .all(|letter| letter.is_ascii_alphanumeric() || letter == '-')
        })
}

fn domain_problem(domain: &str) -> Option<String> {
    if domain.is_empty() {
        return Some(String::from("the domain is empty"));
    }
    if domain
        .contains(|letter: char| letter.is_whitespace() || matches!(letter, ':' | '/' | '?' | '#'))
    {
        return Some(format!(
            "the domain {domain} must have no scheme, port or path"
        ));
    }
    (!domain_shape(domain)).then(|| {
        format!("the domain {domain} is no domain name such as cdn.example.com or *.example.com")
    })
}

/// Whether `host` is released by `permissions.network`: exact names, or `*.` for subdomains.
#[must_use]
pub fn host_allowed(network: &[String], host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return false;
    }
    network.iter().any(|domain| {
        let domain = domain.trim_end_matches('.').to_ascii_lowercase();
        match domain.strip_prefix("*.") {
            Some(base) => host
                .strip_suffix(base)
                .is_some_and(|sub| sub.len() > 1 && sub.ends_with('.')),
            None => host == domain,
        }
    })
}

/// Every field of a stored theme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledThemeRecord {
    pub manifest: ThemeManifest,
    pub installed_at: Timestamp,
    pub updated_at: Timestamp,
    pub version: Version,
}

/// A theme package that is installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledTheme(InstalledThemeRecord);

impl InstalledTheme {
    #[must_use]
    pub const fn install(manifest: ThemeManifest, now: Timestamp) -> Self {
        Self(InstalledThemeRecord {
            manifest,
            installed_at: now,
            updated_at: now,
            version: Version::INITIAL,
        })
    }

    /// The same theme replaced by another package.
    #[must_use]
    pub fn reinstalled(&self, manifest: ThemeManifest, now: Timestamp) -> Self {
        Self(InstalledThemeRecord {
            manifest,
            installed_at: self.0.installed_at,
            updated_at: now.max(self.0.updated_at),
            version: self.0.version.next(),
        })
    }

    #[must_use]
    pub const fn record(&self) -> &InstalledThemeRecord {
        &self.0
    }

    #[must_use]
    pub fn id(&self) -> &ThemeId {
        self.0.manifest.id()
    }

    /// For stores only: trusts the record as stored.
    #[must_use]
    pub const fn restore(record: InstalledThemeRecord) -> Self {
        Self(record)
    }
}

/// Why a package file or a requested path is refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PackageError {
    #[error("unsafe path in the package: {0}")]
    UnsafePath(String),
    #[error("file type not allowed: {0}")]
    ForbiddenFileType(String),
    #[error("the package has no {0}")]
    MissingFile(String),
    #[error("{STYLE_FILE} is not allowed: {0}")]
    ForbiddenCss(String),
    #[error("{STYLE_FILE} is not valid UTF-8")]
    CssEncoding,
}

/// The path of a package entry as segments: relative, plain names only.
pub fn check_entry_path(name: &str) -> Result<Vec<&str>, PackageError> {
    let unsafe_path = || PackageError::UnsafePath(name.to_owned());
    let trimmed = name.strip_suffix('/').unwrap_or(name);
    if trimmed.is_empty()
        || trimmed.len() > MAX_ENTRY_PATH
        || trimmed.starts_with('/')
        || trimmed.contains(['\\', ':'])
        || trimmed.chars().any(char::is_control)
    {
        return Err(unsafe_path());
    }
    trimmed
        .split('/')
        .map(|segment| {
            // Windows drops a trailing dot or space, so such names could alias others
            (!segment.is_empty() && !segment.ends_with(['.', ' ']))
                .then_some(segment)
                .ok_or_else(unsafe_path)
        })
        .collect()
}

fn extension(path: &str) -> Option<String> {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
}

#[must_use]
pub fn is_claude_path(path: &str) -> bool {
    path.split('/').next() == Some(CLAUDE_DIR)
}

/// Code, style, data, images, fonts and sounds; `ts` and `tsx` only below `claude/`.
#[must_use]
pub fn is_allowed_extension(path: &str) -> bool {
    extension(path).is_some_and(|extension| {
        ALLOWED_EXTENSIONS.contains(&extension.as_str())
            || (CLAUDE_EXTENSIONS.contains(&extension.as_str()) && is_claude_path(path))
    })
}

/// Package files must not hide: a name with a leading dot is refused.
#[must_use]
pub fn is_hidden(path: &str) -> bool {
    path.split('/').any(|segment| segment.starts_with('.'))
}

/// A Claude Code theme needs its hooks file; without one nothing may be below `claude/`.
pub fn check_claude_files<'a>(
    manifest: &ThemeManifest,
    paths: impl IntoIterator<Item = &'a str>,
) -> Result<(), FieldError> {
    let (mut has_claude, mut has_hooks) = (false, false);
    for path in paths {
        if is_claude_path(path) {
            has_claude = true;
            has_hooks |= path == CLAUDE_HOOKS_FILE;
        }
    }
    let message = if manifest.record().claude_theme && !has_hooks {
        format!("claudeTheme is on but the package has no {CLAUDE_HOOKS_FILE}")
    } else if !manifest.record().claude_theme && has_claude {
        format!("the package has {CLAUDE_DIR}/ but claudeTheme is not true")
    } else {
        return Ok(());
    };
    Err(FieldError::new("claudeTheme", message))
}

/// Refuses what a browser would load from elsewhere: escapes, `@import`, `image-set()` and the
/// like, and every `url()` that is not relative or `data:`; the text must be well formed.
pub fn check_theme_css(css: &str) -> Result<(), PackageError> {
    crate::css_check::check_css(css, true).map_err(PackageError::ForbiddenCss)
}

/// The segments of a requested file path inside an installed theme, plain names only.
pub fn asset_segments(path: &str) -> Result<Vec<&str>, PackageError> {
    if path.ends_with('/') {
        return Err(PackageError::UnsafePath(path.to_owned()));
    }
    check_entry_path(path)
}

/// Content type of a theme file by its extension; code and style have fixed types.
#[must_use]
pub fn content_type(path: &str) -> &'static str {
    match extension(path).as_deref() {
        Some("css") => "text/css; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        Some("mp3") => "audio/mpeg",
        Some("ogg") => "audio/ogg",
        Some("wav") => "audio/wav",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft() -> ManifestDraft {
        ManifestDraft {
            id: Some(String::from("night-blue")),
            name: Some(String::from("Night Blue")),
            version: Some(String::from("1.2.3")),
            author: Some(String::from("Ada")),
            sdk_version: Some(String::from("^1.3")),
            color_scheme: Some(String::from("dark")),
            ..ManifestDraft::default()
        }
    }

    fn fields(draft: ManifestDraft) -> Vec<String> {
        ThemeManifest::build(draft, Vec::new())
            .unwrap_err()
            .into_iter()
            .map(|error| error.field)
            .collect()
    }

    #[test]
    fn a_complete_manifest_is_accepted() {
        let manifest = ThemeManifest::build(draft(), Vec::new()).unwrap();
        assert_eq!(manifest.id().as_str(), "night-blue");
        assert_eq!(manifest.record().color_scheme, ColorScheme::Dark);
        assert!(manifest.record().settings.is_none());
    }

    #[test]
    fn theme_ids_are_short_lowercase_names() {
        for id in ["ab", "night-blue", "a1-b2", &"a".repeat(40)] {
            assert!(ThemeId::parse(id).is_ok(), "{id}");
        }
        for id in [
            "a",
            "Night",
            "night_blue",
            "../x",
            "a b",
            "",
            &"a".repeat(41),
        ] {
            assert_eq!(ThemeId::parse(id), Err(ThemeError::InvalidId), "{id}");
        }
        assert!(ThemeId::parse("light").unwrap().is_builtin());
        assert!(!ThemeId::parse("night-blue").unwrap().is_builtin());
    }

    #[test]
    fn every_problem_of_a_manifest_is_reported_in_field_order() {
        let draft = ManifestDraft {
            id: Some(String::from("BAD")),
            name: Some(String::from(" ")),
            version: Some(String::from("one")),
            author: Some(String::new()),
            sdk_version: Some(String::from("nope")),
            color_scheme: Some(String::from("blue")),
            slots: vec![(0, String::from("Shell")), (1, String::from("Shell"))],
            network: vec![(0, String::from("https://a.b/c"))],
            ..ManifestDraft::default()
        };
        assert_eq!(
            fields(draft),
            [
                "id",
                "name",
                "version",
                "author",
                "sdkVersion",
                "colorScheme",
                "slots[1]",
                "permissions.network[0]"
            ]
        );
    }

    #[test]
    fn missing_fields_are_named_once() {
        let errors = ThemeManifest::build(ManifestDraft::default(), Vec::new()).unwrap_err();
        let names: Vec<&str> = errors.iter().map(|error| error.field.as_str()).collect();
        assert_eq!(
            names,
            [
                "id",
                "name",
                "version",
                "author",
                "sdkVersion",
                "colorScheme"
            ]
        );
        assert_eq!(errors[0].message, "id is missing");
    }

    #[test]
    fn a_wrong_typed_field_reported_earlier_is_not_reported_as_missing() {
        let mut draft = draft();
        draft.name = None;
        let earlier = vec![FieldError::new("name", "name must be text")];
        let errors = ThemeManifest::build(draft, earlier).unwrap_err();
        assert_eq!(errors, [FieldError::new("name", "name must be text")]);
    }

    #[test]
    fn a_claude_theme_needs_an_sdk_range_from_1_3() {
        let mut claude = draft();
        claude.claude_theme = true;
        claude.sdk_version = Some(String::from("^1.0"));
        assert_eq!(fields(claude.clone()), ["sdkVersion"]);
        claude.sdk_version = Some(String::from(">=1.3"));
        assert!(ThemeManifest::build(claude, Vec::new()).is_ok());
    }

    #[test]
    fn network_domains_have_no_scheme_port_or_path() {
        for domain in ["cdn.example.com", "*.example.com", "a-b.example.org"] {
            assert_eq!(domain_problem(domain), None, "{domain}");
        }
        for domain in [
            "",
            "example",
            "http://a.b",
            "a.b:80",
            "a.b/c",
            "*.",
            "a..b",
            "a b.c",
        ] {
            assert!(domain_problem(domain).is_some(), "{domain}");
        }
    }

    #[test]
    fn hosts_match_exact_names_or_subdomains_only() {
        let network = vec![String::from("example.com"), String::from("*.cdn.test")];
        assert!(host_allowed(&network, "Example.com."));
        assert!(host_allowed(&network, "a.cdn.test"));
        assert!(!host_allowed(&network, "cdn.test"));
        assert!(!host_allowed(&network, "evilexample.com"));
        assert!(!host_allowed(&network, "evil-cdn.test"));
        assert!(!host_allowed(&network, ""));
    }

    #[test]
    fn settings_are_checked_by_key_label_and_options() {
        let mut draft = draft();
        draft.settings = Some(BTreeMap::from([
            (
                String::from("1bad"),
                SettingSpec::Boolean {
                    label: String::from(" "),
                    hint: None,
                    default: true,
                },
            ),
            (
                String::from("mode"),
                SettingSpec::Select {
                    label: String::from("Mode"),
                    hint: None,
                    options: vec![
                        SelectOption {
                            value: String::from("a"),
                            label: String::from("A"),
                        },
                        SelectOption {
                            value: String::from("a"),
                            label: String::from("Again"),
                        },
                    ],
                    default: String::from("z"),
                },
            ),
        ]));
        assert_eq!(
            fields(draft),
            [
                "settingsSchema.1bad",
                "settingsSchema.1bad.label",
                "settingsSchema.mode.options[1]",
                "settingsSchema.mode.default"
            ]
        );
    }

    #[test]
    fn reinstalling_keeps_the_first_install_time_and_counts_up() {
        let manifest = ThemeManifest::build(draft(), Vec::new()).unwrap();
        let first = InstalledTheme::install(manifest.clone(), Timestamp::from_millis(5));
        let second = first.reinstalled(manifest, Timestamp::from_millis(9));
        assert_eq!(second.record().installed_at, Timestamp::from_millis(5));
        assert_eq!(second.record().updated_at, Timestamp::from_millis(9));
        assert_eq!(second.record().version, Version::from_stored(2));
    }

    #[test]
    fn entry_paths_must_be_relative_plain_names() {
        assert_eq!(
            check_entry_path("assets/fonts/a.woff2").unwrap(),
            ["assets", "fonts", "a.woff2"]
        );
        assert_eq!(check_entry_path("assets/").unwrap(), ["assets"]);
        for path in [
            "",
            "/etc/passwd",
            "../x",
            "a/../b",
            "a//b",
            "./a",
            "a\\b",
            "C:/x",
            "a:b",
            "a\u{0}b",
            "a\nb",
            "claude./hooks/hooks.json",
            "assets /a.png",
            "a.png.",
            &"a/".repeat(MAX_ENTRY_PATH),
        ] {
            assert!(check_entry_path(path).is_err(), "{path:?}");
        }
    }

    #[test]
    fn requested_asset_paths_refuse_folders_and_escapes() {
        assert_eq!(asset_segments("theme.css").unwrap(), ["theme.css"]);
        for path in [
            "",
            "assets/",
            "../manifest.json",
            "a/./b",
            "..",
            "a\\b",
            "/x",
        ] {
            assert!(asset_segments(path).is_err(), "{path:?}");
        }
    }

    #[test]
    fn file_types_follow_the_allow_list() {
        assert!(is_allowed_extension("theme.css"));
        assert!(is_allowed_extension("assets/FONT.WOFF2"));
        assert!(!is_allowed_extension("run.exe"));
        assert!(!is_allowed_extension("page.html"));
        assert!(!is_allowed_extension("noext"));
        assert!(!is_allowed_extension("src/main.ts"));
        assert!(is_allowed_extension("claude/hooks/look.ts"));
        assert!(is_hidden(".git/config"));
        assert!(is_hidden("assets/.env"));
        assert!(!is_hidden("assets/a.png"));
    }

    #[test]
    fn claude_files_follow_the_manifest() {
        let mut manifest = draft();
        manifest.claude_theme = true;
        let with = ThemeManifest::build(manifest, Vec::new()).unwrap();
        let without = ThemeManifest::build(draft(), Vec::new()).unwrap();
        assert!(check_claude_files(&with, [CLAUDE_HOOKS_FILE, "theme.css"]).is_ok());
        assert!(check_claude_files(&with, ["theme.css"]).is_err());
        assert!(check_claude_files(&without, ["theme.css"]).is_ok());
        assert!(check_claude_files(&without, [CLAUDE_HOOKS_FILE]).is_err());
    }

    #[test]
    fn css_may_not_import_or_reach_out() {
        assert!(check_theme_css("a { color: red }").is_ok());
        assert!(check_theme_css("@font-face { src: url(assets/a.woff2) }").is_ok());
        assert!(check_theme_css("a { background: url( 'assets/a.png' ) }").is_ok());
        assert!(check_theme_css("a { background: url(data:image/png;base64,AAAA) }").is_ok());
        assert!(check_theme_css("/* @import 'x'; url(http://x) */ a {}").is_ok());
        for css in [
            "@IMPORT url(a.css);",
            "a { background: url(http://evil.test/a.png) }",
            "a { background: url('//evil.test/a.png') }",
            "a { background: url(/abs.png) }",
            "a { background: URL( \"https://evil.test\" ) }",
            "a { background: url(a\\b.png) }",
            "a { background: url(file:x) }",
            "a{content:\"/*\"} b{background:url(https://evil/x)} c{content:\"*/\"}",
            "a { background: \\75 rl(https://evil/x) }",
            "@\\69mport 'https://evil/x.css';",
            "a { background: image-set(\"https://evil/x.png\" 1x) }",
            "a { background: -webkit-image-set(\"https://evil/x.png\" 1x) }",
            "a{background:url(x/*)}b{background:url(https://evil/p)}c{background:url(*/)}",
            "a{content:\"x\r\"/* \"}b{background:url(https://evil)}c{d:*/}",
            "a{content:\"x\u{c}\"/* \"}b{background:url(https://evil)}c{d:*/}",
            "@font-face { src: src(\"https://evil/f.woff2\") }",
            "a{background:url(\"data:x
);}
b{background:url(https://evil.test/x.png)}",
            "a{background:url(data:x
p{content:\")\"}/*\"} y{background:url(https://evil.test/x.png)} */",
            "@media print {",
            "a { background: image(\"https://evil/x.png\") }",
        ] {
            assert!(
                matches!(check_theme_css(css), Err(PackageError::ForbiddenCss(_))),
                "{css}"
            );
        }
    }

    #[test]
    fn start_color_and_preview_are_checked() {
        for color in ["red", "#12", "#123456789", "#12g", "url(x)", ""] {
            let mut manifest = draft();
            manifest.start_color = Some(color.to_owned());
            assert_eq!(fields(manifest), ["startColor"], "{color}");
        }
        for color in ["#abc", "#0c0c0e", "#0C0C0EFF"] {
            let mut manifest = draft();
            manifest.start_color = Some(color.to_owned());
            assert!(
                ThemeManifest::build(manifest, Vec::new()).is_ok(),
                "{color}"
            );
        }
        for preview in ["../a.png", "/a.png", "a.exe", "a", "assets/", "a\\b.png"] {
            let mut manifest = draft();
            manifest.preview = Some(preview.to_owned());
            assert_eq!(fields(manifest), ["preview"], "{preview}");
        }
        let mut manifest = draft();
        manifest.preview = Some(String::from("assets/preview.PNG"));
        assert!(ThemeManifest::build(manifest, Vec::new()).is_ok());
    }

    #[test]
    fn long_texts_are_refused_before_the_store() {
        let long = |count| "a".repeat(count);
        let mut manifest = draft();
        manifest.name = Some(long(1001));
        manifest.author = Some(long(1001));
        let mut found = fields(manifest);
        found.dedup();
        assert_eq!(found, ["name", "author"]);
        let mut manifest = draft();
        manifest.sdk_version = Some(format!("^1.0{}", ", <2.0".repeat(20)));
        assert!(manifest.sdk_version.as_deref().is_some_and(|range| {
            range.len() > 100 && semver::VersionReq::parse(range).is_ok()
        }));
        assert_eq!(fields(manifest), ["sdkVersion"]);
        let mut manifest = draft();
        manifest.version = Some(format!("1.0.0-{}", long(100)));
        assert_eq!(fields(manifest), ["version"]);
        let mut manifest = draft();
        manifest.name = Some(long(1000));
        assert!(ThemeManifest::build(manifest, Vec::new()).is_ok());
    }

    #[test]
    fn content_types_are_fixed_by_extension() {
        assert_eq!(content_type("theme.css"), "text/css; charset=utf-8");
        assert_eq!(content_type("main.JS"), "text/javascript; charset=utf-8");
        assert_eq!(content_type("a.png"), "image/png");
        assert_eq!(content_type("a.unknown"), "application/octet-stream");
        assert_eq!(content_type("noext"), "application/octet-stream");
    }
}
