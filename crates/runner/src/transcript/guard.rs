//! Which files may be read as transcripts: regular files below a Claude config folder's `projects`.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

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
    /// Allows the `projects` folder of each of the config folders `roots`, and any a path names.
    pub(crate) fn new(roots: Vec<PathBuf>) -> Self {
        Self {
            roots,
            fold: RunnerOs::current().ignores_path_case(),
        }
    }

    /// The config folder whose `projects` folder holds `file`: a listed one, else the one it names.
    fn root_of(&self, file: &Path) -> Option<PathBuf> {
        let wanted = parts(file, self.fold);
        let listed = self.roots.iter().find(|root| {
            let allowed = parts(&root.join(PROJECTS), self.fold);
            wanted.len() > allowed.len() && wanted.starts_with(&allowed)
        });
        listed.cloned().or_else(|| self.named_root(file))
    }

    /// Hooks report the transcripts of any config folder: `<root>/projects/<folder>/.../<file>`.
    fn named_root(&self, file: &Path) -> Option<PathBuf> {
        let names: Vec<Component<'_>> = file.components().collect();
        let at = names.iter().enumerate().position(|(index, part)| {
            let Component::Normal(name) = part else {
                return false;
            };
            let name = name.to_string_lossy();
            let is_projects = if self.fold {
                name.eq_ignore_ascii_case(PROJECTS)
            } else {
                name == PROJECTS
            };
            // a named config folder, then a project folder and a file
            is_projects
                && names[..index]
                    .iter()
                    .any(|above| matches!(above, Component::Normal(_)))
                && names.len() > index + 2
        })?;
        Some(names[..at].iter().collect())
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
    fn a_listed_config_folder_counts_and_a_file_beside_projects_does_not() {
        let (_dir, root, file) = config();
        let other = tempfile::tempdir().unwrap();
        let guard = TranscriptGuard::new(vec![other.path().to_path_buf(), root.clone()]);
        assert!(guard.file(&text(&file)).unwrap().is_some());
        let loose = root.join("s.jsonl");
        fs::write(
            &loose, "{}
",
        )
        .unwrap();
        let none = TranscriptGuard::new(Vec::new());
        for guard in [&guard, &none] {
            assert_eq!(
                guard.file(&text(&loose)).unwrap_err().code(),
                "transcript-path-invalid"
            );
        }
    }

    #[test]
    fn a_transcript_of_another_config_folder_passes_by_its_shape() {
        let (_dir, root, file) = config();
        let guard = TranscriptGuard::new(Vec::new());
        let found = guard.file(&text(&file)).unwrap().unwrap();
        assert_eq!(found, fs::canonicalize(&file).unwrap());
        let nested = root.join("projects").join("demo").join("deep");
        fs::create_dir_all(&nested).unwrap();
        let sub = nested.join("agent.jsonl");
        fs::write(
            &sub, "{}
",
        )
        .unwrap();
        assert!(guard.file(&text(&sub)).unwrap().is_some());
    }

    #[test]
    fn a_path_without_the_projects_shape_is_refused() {
        let (_dir, root, _file) = config();
        let guard = TranscriptGuard::new(Vec::new());
        let history = root.join("history.jsonl");
        let direct = root.join("projects").join("s.jsonl");
        let other = root.join("other").join("demo");
        fs::create_dir_all(&other).unwrap();
        let strange = other.join("s.jsonl");
        for path in [&history, &direct, &strange] {
            fs::write(
                path, "{}
",
            )
            .unwrap();
            let error = guard.file(&text(path)).unwrap_err();
            assert_eq!(error.code(), "transcript-path-invalid", "{path:?}");
        }
        let bare = Path::new("/projects/demo/s.jsonl");
        assert_eq!(
            guard.file(&text(bare)).unwrap_err().code(),
            "transcript-path-invalid"
        );
    }

    #[test]
    fn a_unc_path_stays_refused_even_in_the_projects_shape() {
        let guard = TranscriptGuard::new(Vec::new());
        for path in [
            "\\\\host\\share\\.claude\\projects\\demo\\s.jsonl",
            "//host/share/.claude/projects/demo/s.jsonl",
            "\\\\?\\UNC\\host\\share\\.claude\\projects\\demo\\s.jsonl",
        ] {
            assert_eq!(
                guard.file(path).unwrap_err().code(),
                "transcript-path-invalid",
                "{path}"
            );
        }
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
        for roots in [vec![root], Vec::new()] {
            let error = TranscriptGuard::new(roots).file(&text(&link)).unwrap_err();
            assert_eq!(error.code(), "transcript-path-invalid");
        }
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
        for roots in [vec![root], Vec::new()] {
            let guard = TranscriptGuard::new(roots);
            let error = guard.file(&text(&link.join("s.jsonl"))).unwrap_err();
            assert_eq!(error.code(), "transcript-path-invalid");
        }
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
