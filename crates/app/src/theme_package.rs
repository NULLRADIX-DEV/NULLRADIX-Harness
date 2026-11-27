//! Checks the files of a theme package before anything is stored.

use std::collections::{BTreeMap, HashSet};

use nullradix_domain::{
    FieldError, MANIFEST_FILE, MAX_ENTRIES, MAX_FILE_BYTES, MAX_PACKAGE_BYTES, ManifestDraft,
    PackageError, STYLE_FILE, SelectOption, SettingSpec, ThemeManifest, check_claude_files,
    check_entry_path, check_theme_css, is_allowed_extension, is_hidden,
};
use serde_json::{Map, Value};

use crate::{AppError, FieldProblem};

/// One file of a package: a relative path with `/` and its content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageFile {
    pub path: String,
    pub data: Vec<u8>,
}

#[must_use]
pub fn package_invalid(message: impl Into<String>) -> AppError {
    AppError::invalid("theme-package-invalid", message)
}

#[must_use]
pub fn manifest_invalid(errors: Vec<FieldError>) -> AppError {
    let message = errors
        .iter()
        .map(|error| format!("{}: {}", error.field, error.message))
        .collect::<Vec<_>>()
        .join("; ");
    let fields = errors
        .into_iter()
        .map(|error| FieldProblem {
            field: error.field,
            message: error.message,
        })
        .collect();
    AppError::invalid("theme-manifest-invalid", message).with_fields(fields)
}

fn field(name: &str, message: impl Into<String>) -> FieldError {
    FieldError::new(name, message)
}

fn text(object: &Map<String, Value>, key: &str, errors: &mut Vec<FieldError>) -> Option<String> {
    match object.get(key) {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) => Some(value.clone()),
        Some(_) => {
            errors.push(field(key, format!("{key} must be text")));
            None
        }
    }
}

/// A list of texts with their positions; entries of another type are errors and dropped.
fn text_list(
    value: Option<&Value>,
    name: &str,
    errors: &mut Vec<FieldError>,
) -> Vec<(usize, String)> {
    match value {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| {
                if let Value::String(value) = item {
                    return Some((index, value.clone()));
                }
                errors.push(field(
                    format!("{name}[{index}]").as_str(),
                    "the entry must be text",
                ));
                None
            })
            .collect(),
        Some(_) => {
            errors.push(field(name, format!("{name} must be a list of texts")));
            Vec::new()
        }
    }
}

fn optional_text(entry: &Map<String, Value>, key: &str) -> Result<Option<String>, ()> {
    match entry.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(()),
    }
}

fn setting_spec(entry: &Value) -> Option<SettingSpec> {
    let entry = entry.as_object()?;
    let label = entry.get("label")?.as_str()?.to_owned();
    let hint = optional_text(entry, "hint").ok()?;
    match entry.get("type")?.as_str()? {
        "boolean" => Some(SettingSpec::Boolean {
            label,
            hint,
            default: entry.get("default")?.as_bool()?,
        }),
        "select" => {
            let options = entry
                .get("options")?
                .as_array()?
                .iter()
                .map(|option| {
                    let option = option.as_object()?;
                    Some(SelectOption {
                        value: option.get("value")?.as_str()?.to_owned(),
                        label: option.get("label")?.as_str()?.to_owned(),
                    })
                })
                .collect::<Option<Vec<_>>>()?;
            Some(SettingSpec::Select {
                label,
                hint,
                options,
                default: entry.get("default")?.as_str()?.to_owned(),
            })
        }
        _ => None,
    }
}

fn settings(
    value: Option<&Value>,
    errors: &mut Vec<FieldError>,
) -> Option<BTreeMap<String, SettingSpec>> {
    let entries = match value {
        None | Some(Value::Null) => return None,
        Some(Value::Object(entries)) => entries,
        Some(_) => {
            errors.push(field("settingsSchema", "settingsSchema must be an object"));
            return None;
        }
    };
    let mut schema = BTreeMap::new();
    for (key, entry) in entries {
        match setting_spec(entry) {
            Some(spec) => {
                schema.insert(key.clone(), spec);
            }
            None => errors.push(field(
                &format!("settingsSchema.{key}"),
                "a setting has type boolean or select, a label and a default",
            )),
        }
    }
    Some(schema)
}

/// Reads and checks the text of a `manifest.json`; all problems come back together.
pub fn parse_manifest(json: &str) -> Result<ThemeManifest, Vec<FieldError>> {
    let value: Value = serde_json::from_str(json)
        .map_err(|error| vec![field("", format!("the manifest is no valid JSON: {error}"))])?;
    let Value::Object(object) = &value else {
        return Err(vec![field("", "the manifest must be a JSON object")]);
    };
    let mut errors = Vec::new();
    let mut draft = ManifestDraft {
        id: text(object, "id", &mut errors),
        name: text(object, "name", &mut errors),
        version: text(object, "version", &mut errors),
        author: text(object, "author", &mut errors),
        sdk_version: text(object, "sdkVersion", &mut errors),
        color_scheme: text(object, "colorScheme", &mut errors),
        start_color: text(object, "startColor", &mut errors),
        preview: text(object, "preview", &mut errors),
        slots: text_list(object.get("slots"), "slots", &mut errors),
        ..ManifestDraft::default()
    };
    match object.get("permissions") {
        None | Some(Value::Null) => {}
        Some(Value::Object(permissions)) => {
            draft.network = text_list(
                permissions.get("network"),
                "permissions.network",
                &mut errors,
            );
        }
        Some(_) => errors.push(field("permissions", "permissions must be an object")),
    }
    draft.settings = settings(object.get("settingsSchema"), &mut errors);
    draft.claude_theme = match object.get("claudeTheme") {
        None | Some(Value::Null) => false,
        Some(Value::Bool(flag)) => *flag,
        Some(_) => {
            errors.push(field("claudeTheme", "claudeTheme must be true or false"));
            false
        }
    };
    ThemeManifest::build(draft, errors)
}

fn setting_json(spec: &SettingSpec) -> Value {
    match spec {
        SettingSpec::Boolean {
            label,
            hint,
            default,
        } => {
            let mut entry =
                serde_json::json!({ "type": "boolean", "label": label, "default": default });
            if let Some(hint) = hint {
                entry["hint"] = Value::from(hint.clone());
            }
            entry
        }
        SettingSpec::Select {
            label,
            hint,
            options,
            default,
        } => {
            let options: Vec<Value> = options
                .iter()
                .map(|option| serde_json::json!({ "value": option.value, "label": option.label }))
                .collect();
            let mut entry = serde_json::json!({
                "type": "select", "label": label, "options": options, "default": default
            });
            if let Some(hint) = hint {
                entry["hint"] = Value::from(hint.clone());
            }
            entry
        }
    }
}

/// The manifest as a `manifest.json` file spells it.
#[must_use]
pub fn manifest_json(manifest: &ThemeManifest) -> Value {
    let record = manifest.record();
    let mut json = serde_json::json!({
        "id": record.id.as_str(),
        "name": record.name,
        "version": record.version,
        "author": record.author,
        "sdkVersion": record.sdk_version,
        "colorScheme": record.color_scheme.as_str(),
        "slots": record.slots,
        "permissions": { "network": record.network },
    });
    if let Some(color) = &record.start_color {
        json["startColor"] = Value::from(color.clone());
    }
    if let Some(preview) = &record.preview {
        json["preview"] = Value::from(preview.clone());
    }
    if let Some(settings) = &record.settings {
        let entries: Map<String, Value> = settings
            .iter()
            .map(|(key, spec)| (key.clone(), setting_json(spec)))
            .collect();
        json["settingsSchema"] = Value::Object(entries);
    }
    if record.claude_theme {
        json["claudeTheme"] = Value::Bool(true);
    }
    json
}

fn package_error(error: &PackageError) -> AppError {
    package_invalid(error.to_string())
}

/// Checks paths, file types, sizes, the manifest, `theme.css` and the Claude files.
pub fn check_package(files: &[PackageFile], pack: bool) -> Result<ThemeManifest, AppError> {
    if files.len() > MAX_ENTRIES {
        return Err(package_invalid(format!(
            "the package has more than {MAX_ENTRIES} files"
        )));
    }
    let mut seen = HashSet::new();
    let mut total: u64 = 0;
    for file in files {
        check_entry_path(&file.path).map_err(|error| package_error(&error))?;
        // lowercased: case-insensitive disks would merge `a.css` and `A.css`
        if !seen.insert(file.path.to_lowercase()) {
            return Err(package_error(&PackageError::UnsafePath(file.path.clone())));
        }
        if pack && is_hidden(&file.path) {
            return Err(package_invalid(format!(
                "hidden files and folders are not allowed: {}",
                file.path
            )));
        }
        if !is_allowed_extension(&file.path) {
            return Err(package_error(&PackageError::ForbiddenFileType(
                file.path.clone(),
            )));
        }
        let size = file.data.len() as u64;
        if size > MAX_FILE_BYTES {
            return Err(package_invalid(format!(
                "{} is larger than {MAX_FILE_BYTES} bytes",
                file.path
            )));
        }
        total += size;
        if total > MAX_PACKAGE_BYTES {
            return Err(package_invalid(format!(
                "the files are larger than {MAX_PACKAGE_BYTES} bytes together"
            )));
        }
    }
    let find = |name: &str| {
        files
            .iter()
            .find(|file| file.path == name)
            .map(|file| file.data.as_slice())
            .ok_or_else(|| package_error(&PackageError::MissingFile(name.to_owned())))
    };
    let manifest =
        parse_manifest(&String::from_utf8_lossy(find(MANIFEST_FILE)?)).map_err(manifest_invalid)?;
    let css = std::str::from_utf8(find(STYLE_FILE)?)
        .map_err(|_| package_error(&PackageError::CssEncoding))?;
    check_theme_css(css).map_err(|error| package_error(&error))?;
    check_claude_files(&manifest, files.iter().map(|file| file.path.as_str()))
        .map_err(|error| manifest_invalid(vec![error]))?;
    Ok(manifest)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const MANIFEST: &str = r##"{
        "id": "night-blue", "name": "Night Blue", "version": "1.0.0", "author": "Ada",
        "sdkVersion": "^2.0", "colorScheme": "dark", "startColor": "#101018",
        "slots": [], "permissions": { "network": [] }
    }"##;

    pub(crate) fn file(path: &str, data: &str) -> PackageFile {
        PackageFile {
            path: path.to_owned(),
            data: data.as_bytes().to_vec(),
        }
    }

    pub(crate) fn package() -> Vec<PackageFile> {
        vec![
            file("manifest.json", MANIFEST),
            file("theme.css", ":root { --accent: #6c63ff }"),
        ]
    }

    #[test]
    fn a_minimal_package_is_accepted() {
        let manifest = check_package(&package(), false).unwrap();
        assert_eq!(manifest.id().as_str(), "night-blue");
    }

    #[test]
    fn the_manifest_reports_every_field_problem_with_its_name() {
        let json = r#"{ "id": 5, "slots": "x", "permissions": [], "claudeTheme": "yes" }"#;
        let errors = parse_manifest(json).unwrap_err();
        let fields: Vec<&str> = errors.iter().map(|error| error.field.as_str()).collect();
        assert_eq!(
            fields,
            [
                "id",
                "name",
                "version",
                "author",
                "sdkVersion",
                "colorScheme",
                "slots",
                "permissions",
                "claudeTheme"
            ]
        );
        assert_eq!(errors[0].message, "id must be text");
    }

    #[test]
    fn json_that_is_no_manifest_is_refused_as_a_whole() {
        for json in ["", "{", "[]", "\"x\"", "null"] {
            let errors = parse_manifest(json).unwrap_err();
            assert_eq!(errors.len(), 1, "{json}");
            assert_eq!(errors[0].field, "");
        }
    }

    #[test]
    fn list_entries_of_another_type_are_reported_with_their_index() {
        let json = MANIFEST.replace(r#""slots": []"#, r#""slots": ["Shell", 3]"#);
        let errors = parse_manifest(&json).unwrap_err();
        assert_eq!(
            errors,
            [FieldError::new("slots[1]", "the entry must be text")]
        );
    }

    #[test]
    fn settings_are_read_and_misshaped_ones_named_by_key() {
        let json = MANIFEST.replace(
            r#""slots": []"#,
            r#""slots": [], "settingsSchema": {
                "dim": { "type": "boolean", "label": "Dim", "default": true },
                "mode": { "type": "select", "label": "Mode", "default": "a",
                          "options": [{ "value": "a", "label": "A" }] },
                "odd": { "type": "slider" }
            }"#,
        );
        let errors = parse_manifest(&json).unwrap_err();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].field, "settingsSchema.odd");
        let fine = json.replace(
            r#""odd": { "type": "slider" }"#,
            r#""odd": { "type": "boolean", "label": "O", "default": false }"#,
        );
        let manifest = parse_manifest(&fine).unwrap();
        assert_eq!(manifest.record().settings.as_ref().unwrap().len(), 3);
    }

    #[test]
    fn a_manifest_round_trips_through_its_file_form() {
        let json = MANIFEST.replace(
            r#""slots": []"#,
            r#""slots": ["Progress"], "preview": "assets/preview.png", "claudeTheme": true,
               "settingsSchema": {
                "dim": { "type": "boolean", "label": "Dim", "hint": "less light", "default": true },
                "mode": { "type": "select", "label": "Mode", "default": "a",
                          "options": [{ "value": "a", "label": "A" }] } }"#,
        );
        let manifest = parse_manifest(&json).unwrap();
        let again = parse_manifest(&manifest_json(&manifest).to_string()).unwrap();
        assert_eq!(again, manifest);
        assert_eq!(manifest_json(&again)["claudeTheme"], true);
    }

    #[test]
    fn missing_core_files_are_named() {
        let error = check_package(&[file("theme.css", "a {}")], false).unwrap_err();
        assert_eq!(error.code(), "theme-package-invalid");
        assert!(error.message().contains("manifest.json"));
        let error = check_package(&[file("manifest.json", MANIFEST)], false).unwrap_err();
        assert!(error.message().contains("theme.css"));
    }

    #[test]
    fn unsafe_paths_and_forbidden_types_are_refused() {
        for path in [
            "../theme.css",
            "/theme.css",
            "a/../theme.css",
            "C:/x.css",
            "a\\b.css",
            "a//b.css",
        ] {
            let mut files = package();
            files.push(file(path, "x"));
            let error = check_package(&files, false).unwrap_err();
            assert_eq!(error.code(), "theme-package-invalid", "{path}");
        }
        for path in ["run.exe", "page.html", "script.sh", "src/main.ts", "noext"] {
            let mut files = package();
            files.push(file(path, "x"));
            let error = check_package(&files, false).unwrap_err();
            assert!(error.message().contains("not allowed"), "{path}");
        }
    }

    #[test]
    fn duplicate_entries_are_refused() {
        let mut files = package();
        files.push(file("theme.css", "b {}"));
        assert!(check_package(&files, false).is_err());
    }

    #[test]
    fn entries_that_differ_only_by_case_are_duplicates() {
        let mut files = package();
        files.push(file("Theme.css", "b {}"));
        assert!(check_package(&files, false).is_err());
    }

    #[test]
    fn size_and_count_limits_hold() {
        let mut files = package();
        files.push(PackageFile {
            path: String::from("assets/big.png"),
            data: vec![0; usize::try_from(MAX_FILE_BYTES).unwrap() + 1],
        });
        assert!(check_package(&files, false).is_err());
        let mut many = package();
        for index in 0..=MAX_ENTRIES {
            many.push(file(&format!("assets/{index}.json"), "{}"));
        }
        assert!(check_package(&many, false).is_err());
        let mut together = package();
        for index in 0..5 {
            together.push(PackageFile {
                path: format!("assets/{index}.png"),
                data: vec![0; usize::try_from(MAX_FILE_BYTES).unwrap()],
            });
        }
        assert!(check_package(&together, false).is_err());
    }

    #[test]
    fn styles_with_imports_or_external_urls_are_refused() {
        for css in [
            "@import 'x.css';",
            "a { background: url(https://x.test/a.png) }",
        ] {
            let files = vec![file("manifest.json", MANIFEST), file("theme.css", css)];
            let error = check_package(&files, false).unwrap_err();
            assert!(error.message().contains("theme.css"), "{css}");
        }
        let binary = vec![
            file("manifest.json", MANIFEST),
            PackageFile {
                path: String::from("theme.css"),
                data: vec![0xff, 0xfe],
            },
        ];
        assert!(check_package(&binary, false).is_err());
    }

    #[test]
    fn a_bad_manifest_becomes_one_error_with_its_fields() {
        let files = vec![
            file("manifest.json", r#"{"id": "A"}"#),
            file("theme.css", "a {}"),
        ];
        let error = check_package(&files, false).unwrap_err();
        assert_eq!(error.code(), "theme-manifest-invalid");
        assert_eq!(error.fields()[0].field, "id");
        assert!(error.message().starts_with("id: "));
    }

    #[test]
    fn the_claude_folder_must_match_the_manifest() {
        let with_dir = {
            let mut files = package();
            files.push(file("claude/hooks/hooks.json", "{}"));
            files
        };
        let error = check_package(&with_dir, false).unwrap_err();
        assert_eq!(error.fields()[0].field, "claudeTheme");
        let json = MANIFEST.replace(r#""slots": []"#, r#""slots": [], "claudeTheme": true"#);
        let mut claude = vec![file("manifest.json", &json), file("theme.css", "a {}")];
        assert!(check_package(&claude, false).is_err());
        claude.push(file("claude/hooks/hooks.json", "{}"));
        claude.push(file("claude/hooks/look.ts", "export {}"));
        assert!(check_package(&claude, false).is_ok());
    }

    #[test]
    fn packing_refuses_hidden_files_which_installing_ignores() {
        let mut files = package();
        files.push(file("assets/.secret.json", "{}"));
        assert!(check_package(&files, false).is_ok());
        let error = check_package(&files, true).unwrap_err();
        assert!(error.message().contains("hidden"));
    }
}
