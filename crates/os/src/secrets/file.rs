use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use serde_json::{Map, Value, json};

use super::{SecretBackend, SecretError};
use crate::files::write_private;

const NAME: &str = "private file";

/// Secrets in one JSON file readable by its owner only.
#[derive(Debug)]
pub struct FileSecrets {
    path: PathBuf,
    guard: Mutex<()>,
}

impl FileSecrets {
    #[must_use]
    pub fn in_dir(dir: &Path) -> Self {
        Self {
            path: dir.join("secrets").join("secrets.json"),
            guard: Mutex::new(()),
        }
    }

    fn damaged(&self) -> SecretError {
        SecretError::Damaged {
            path: self.path.clone(),
        }
    }

    fn failed(source: io::Error) -> SecretError {
        SecretError::Backend {
            backend: NAME,
            source,
        }
    }

    fn read_all(&self) -> Result<BTreeMap<String, String>, SecretError> {
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
            Err(error) => return Err(Self::failed(error)),
        };
        let document: Value = serde_json::from_str(&text).map_err(|_| self.damaged())?;
        let secrets = document
            .get("secrets")
            .and_then(Value::as_object)
            .ok_or_else(|| self.damaged())?;
        secrets
            .iter()
            .map(|(key, value)| {
                let value = value.as_str().ok_or_else(|| self.damaged())?;
                Ok((key.clone(), value.to_owned()))
            })
            .collect()
    }

    fn write_all(&self, secrets: &BTreeMap<String, String>) -> Result<(), SecretError> {
        if let Some(dir) = self.path.parent() {
            crate::files::create_private_dir(dir).map_err(Self::failed)?;
        }
        let entries: Map<String, Value> = secrets
            .iter()
            .map(|(key, value)| (key.clone(), json!(value)))
            .collect();
        let document = json!({ "version": 1, "secrets": entries });
        let bytes = serde_json::to_vec_pretty(&document)
            .map_err(|error| Self::failed(io::Error::other(error)))?;
        write_private(&self.path, &bytes).map_err(Self::failed)
    }
}

impl SecretBackend for FileSecrets {
    fn name(&self) -> &'static str {
        NAME
    }

    fn get(&self, key: &str) -> Result<Option<String>, SecretError> {
        let _guard = self.guard.lock().unwrap_or_else(PoisonError::into_inner);
        Ok(self.read_all()?.remove(key))
    }

    fn put(&self, key: &str, value: &str) -> Result<(), SecretError> {
        let _guard = self.guard.lock().unwrap_or_else(PoisonError::into_inner);
        let mut secrets = self.read_all()?;
        secrets.insert(key.to_owned(), value.to_owned());
        self.write_all(&secrets)
    }

    fn delete(&self, key: &str) -> Result<(), SecretError> {
        let _guard = self.guard.lock().unwrap_or_else(PoisonError::into_inner);
        let mut secrets = self.read_all()?;
        if secrets.remove(key).is_some() {
            self.write_all(&secrets)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, FileSecrets) {
        let dir = tempfile::tempdir().unwrap();
        let secrets = FileSecrets::in_dir(&dir.path().join("data"));
        (dir, secrets)
    }

    #[test]
    fn keeps_replaces_and_deletes_secrets() {
        let (_dir, secrets) = store();
        assert_eq!(secrets.get("a").unwrap(), None);
        secrets.put("a", "one").unwrap();
        secrets.put("b", "other").unwrap();
        secrets.put("a", "two").unwrap();
        assert_eq!(secrets.get("a").unwrap().as_deref(), Some("two"));
        assert_eq!(secrets.get("b").unwrap().as_deref(), Some("other"));
        secrets.delete("a").unwrap();
        secrets.delete("a").unwrap();
        assert_eq!(secrets.get("a").unwrap(), None);
        assert_eq!(secrets.get("b").unwrap().as_deref(), Some("other"));
    }

    #[test]
    fn a_second_store_on_the_same_folder_sees_the_secrets() {
        let (dir, secrets) = store();
        secrets.put("a", "one").unwrap();
        let again = FileSecrets::in_dir(&dir.path().join("data"));
        assert_eq!(again.get("a").unwrap().as_deref(), Some("one"));
    }

    #[test]
    fn deleting_a_missing_secret_creates_no_file() {
        let (dir, secrets) = store();
        secrets.delete("a").unwrap();
        assert!(!dir.path().join("data").exists());
    }

    #[test]
    fn a_damaged_file_is_reported_and_left_alone() {
        let (dir, secrets) = store();
        secrets.put("a", "one").unwrap();
        let path = dir.path().join("data").join("secrets").join("secrets.json");
        fs::write(&path, "{ not json").unwrap();
        assert!(matches!(secrets.get("a"), Err(SecretError::Damaged { .. })));
        assert!(matches!(
            secrets.put("b", "x"),
            Err(SecretError::Damaged { .. })
        ));
        assert_eq!(fs::read_to_string(&path).unwrap(), "{ not json");
        fs::write(&path, r#"{"secrets":{"a":5}}"#).unwrap();
        assert!(matches!(secrets.get("a"), Err(SecretError::Damaged { .. })));
    }

    #[test]
    fn keys_and_values_may_hold_any_text() {
        let (_dir, secrets) = store();
        secrets
            .put("account-token/\"x\"", "line\nbreak \\ \u{e4}")
            .unwrap();
        assert_eq!(
            secrets.get("account-token/\"x\"").unwrap().as_deref(),
            Some("line\nbreak \\ \u{e4}")
        );
    }

    #[cfg(unix)]
    #[test]
    fn the_secrets_folder_is_private_to_its_owner() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, secrets) = store();
        secrets.put("a", "one").unwrap();
        let mode = fs::metadata(dir.path().join("data").join("secrets"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
    }

    #[cfg(windows)]
    #[test]
    fn the_file_and_its_folder_belong_to_the_current_user_alone() {
        let (dir, secrets) = store();
        secrets.put("a", "one").unwrap();
        secrets.put("a", "two").unwrap();
        let user = crate::windows::UserSid::current().unwrap();
        let sid = user.text();
        assert!(sid.starts_with("S-1-5-"), "{sid}");
        // SDDL names the built-in administrator LA instead of its SID
        let name = if sid.ends_with("-500") { "LA" } else { sid };
        let folder = dir.path().join("data").join("secrets");
        for path in [folder.clone(), folder.join("secrets.json")] {
            let acl = crate::windows::dacl_sddl(&path).unwrap();
            // owned by the user, protected, and one entry: full access for that SID, not for OW
            assert!(acl.starts_with(&format!("O:{name}D:P")), "{acl}");
            assert_eq!(acl.matches('(').count(), 1, "{acl}");
            assert!(acl.contains(&format!(";FA;;;{name})")), "{acl}");
            assert!(!acl.contains(";OW)") && !acl.contains(";BA)"), "{acl}");
        }
        assert_eq!(secrets.get("a").unwrap().as_deref(), Some("two"));
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_private_to_its_owner() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, secrets) = store();
        secrets.put("a", "one").unwrap();
        secrets.put("a", "two").unwrap();
        let mode = fs::metadata(dir.path().join("data").join("secrets").join("secrets.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
