//! Which files may be read as transcripts: regular files below a Claude config folder's `projects`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use nullradix_app::AppError;
use nullradix_app::session_detail::{check_transcript_path, transcript_path_invalid};
use nullradix_domain::RunnerOs;

/// Claude Code keeps the transcripts of a config folder in this subfolder.
const PROJECTS: &str = "projects";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TranscriptGuard {
    roots: Vec<PathBuf>,
    fold: bool,
}

/// Parts of a path, folded where the file system ignores case.
fn parts(path: &Path, fold: bool) -> Vec<String> {
    path.components()
        .map(|part| {
            let part = part.as_os_str().to_string_lossy();
            if fold {
                part.to_lowercase()
            } else {
                part.into_owned()
            }
        })
        .collect()
}

#[cfg(windows)]
fn is_link(meta: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const REPARSE_POINT: u32 = 0x400;
    meta.file_attributes() & REPARSE_POINT != 0 || meta.file_type().is_symlink()
}

#[cfg(not(windows))]
fn is_link(meta: &fs::Metadata) -> bool {
    meta.file_type().is_symlink()
}

fn unavailable(error: &io::Error) -> AppError {
    AppError::unavailable("runner-failed", format!("the runner failed: {error}"))
}

impl TranscriptGuard {
    /// Allows the `projects` folder of each of the config folders `roots`.
    pub(crate) fn new(roots: Vec<PathBuf>) -> Self {
        Self {
            roots,
            fold: RunnerOs::current().ignores_path_case(),
        }
    }

    /// The config folder whose `projects` folder holds `file`, by its spelling.
    fn root_of(&self, file: &Path) -> Option<&Path> {
        let wanted = parts(file, self.fold);
        self.roots
            .iter()
            .find(|root| {
                let allowed = parts(&root.join(PROJECTS), self.fold);
                wanted.len() > allowed.len() && wanted.starts_with(&allowed)
            })
            .map(PathBuf::as_path)
    }

    /// The transcript to read; `None` when it is gone or no file; links and strangers fail.
    pub(crate) fn file(&self, path: &str) -> Result<Option<PathBuf>, AppError> {
        check_transcript_path(path)?;
        let file = PathBuf::from(path);
        let Some(root) = self.root_of(&file) else {
            return Err(transcript_path_invalid(path));
        };
        let meta = match fs::symlink_metadata(&file) {
            Ok(meta) => meta,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(unavailable(&error)),
        };
        if is_link(&meta) {
            return Err(transcript_path_invalid(path));
        }
        if !meta.is_file() {
            return Ok(None);
        }
        // a link in a folder above must not lead out of `projects` either
        let (Ok(real), Ok(projects)) = (
            fs::canonicalize(&file),
            fs::canonicalize(root.join(PROJECTS)),
        ) else {
            return Ok(None);
        };
        if !real.starts_with(&projects) {
            return Err(transcript_path_invalid(path));
        }
        Ok(Some(real))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(path: &Path) -> String {
        path.to_string_lossy().into_owned()
    }

    /// A config folder with a transcript in its `projects` folder.
    fn config() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("claude");
        let folder = root.join("projects").join("demo");
        fs::create_dir_all(&folder).unwrap();
        let file = folder.join("s.jsonl");
        fs::write(&file, "{}\n").unwrap();
        (dir, root, file)
    }

    #[test]
    fn a_transcript_below_projects_of_a_config_folder_passes() {
        let (_dir, root, file) = config();
        let guard = TranscriptGuard::new(vec![root.clone()]);
        let found = guard.file(&text(&file)).unwrap().unwrap();
        assert_eq!(found, fs::canonicalize(&file).unwrap());
        let gone = file.with_file_name("gone.jsonl");
        assert_eq!(guard.file(&text(&gone)).unwrap(), None);
        let folder = file.with_file_name("folder.jsonl");
        fs::create_dir(&folder).unwrap();
        assert_eq!(guard.file(&text(&folder)).unwrap(), None);
    }

    #[test]
    fn any_listed_config_folder_counts_and_others_do_not() {
        let (_dir, root, file) = config();
        let other = tempfile::tempdir().unwrap();
        let guard = TranscriptGuard::new(vec![other.path().to_path_buf(), root]);
        assert!(guard.file(&text(&file)).unwrap().is_some());
        let none = TranscriptGuard::new(vec![other.path().to_path_buf()]);
        assert_eq!(
            none.file(&text(&file)).unwrap_err().code(),
            "transcript-path-invalid"
        );
        assert_eq!(
            TranscriptGuard::new(Vec::new())
                .file(&text(&file))
                .unwrap_err()
                .code(),
            "transcript-path-invalid"
        );
    }

    #[test]
    fn files_beside_projects_or_in_it_directly_are_refused() {
        let (_dir, root, _file) = config();
        let guard = TranscriptGuard::new(vec![root.clone()]);
        let beside = root.join("history.jsonl");
        fs::write(&beside, "{}\n").unwrap();
        let outside = root.join("projects-extra");
        fs::create_dir_all(&outside).unwrap();
        let near = outside.join("s.jsonl");
        fs::write(&near, "{}\n").unwrap();
        for path in [beside, near, root.join("projects")] {
            let error = guard.file(&text(&path)).unwrap_err();
            assert_eq!(error.code(), "transcript-path-invalid", "{path:?}");
        }
    }

    #[test]
    fn a_file_that_is_a_link_is_refused() {
        let (_dir, root, file) = config();
        let secret = root.join("secret.jsonl");
        fs::write(&secret, "{}\n").unwrap();
        let link = file.with_file_name("link.jsonl");
        if !link_to(&secret, &link) {
            return;
        }
        let guard = TranscriptGuard::new(vec![root]);
        let error = guard.file(&text(&link)).unwrap_err();
        assert_eq!(error.code(), "transcript-path-invalid");
    }

    #[test]
    fn a_folder_that_is_a_link_out_of_projects_is_refused() {
        let (_dir, root, _file) = config();
        let secret = root.join("secrets");
        fs::create_dir_all(&secret).unwrap();
        fs::write(secret.join("s.jsonl"), "{}\n").unwrap();
        let link = root.join("projects").join("escape");
        if !link_to(&secret, &link) {
            return;
        }
        let guard = TranscriptGuard::new(vec![root]);
        let error = guard.file(&text(&link.join("s.jsonl"))).unwrap_err();
        assert_eq!(error.code(), "transcript-path-invalid");
    }

    /// Makes `link` point at `target`; false when this machine may not create links.
    fn link_to(target: &Path, link: &Path) -> bool {
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(target, link);
        #[cfg(windows)]
        let made = if target.is_dir() {
            std::os::windows::fs::symlink_dir(target, link)
        } else {
            std::os::windows::fs::symlink_file(target, link)
        };
        made.is_ok()
    }

    #[test]
    fn spelling_the_folder_in_other_case_follows_the_file_system() {
        let (_dir, root, file) = config();
        let guard = TranscriptGuard::new(vec![root]);
        let shouted = text(&file).replace("projects", "PROJECTS");
        let found = guard.file(&shouted);
        if RunnerOs::current().ignores_path_case() {
            assert!(found.unwrap().is_some());
        } else {
            assert_eq!(found.unwrap_err().code(), "transcript-path-invalid");
        }
    }
}
