use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use anyhow::{Context, Result, bail};
use sqlx::migrate::Migrator;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};

const STORE: &str = "nullradix-store";

fn query_cache(root: &Path) -> PathBuf {
    root.join("crates/store/.sqlx")
}

/// Regenerates the offline query cache of the store crate.
pub(crate) fn prepare(root: &Path) -> Result<ExitCode> {
    let cache = query_cache(root);
    if cache.exists() {
        fs::remove_dir_all(&cache).with_context(|| format!("cannot clear {}", cache.display()))?;
    }
    generate(root, &cache)?;
    println!("query cache written to {}", cache.display());
    Ok(ExitCode::SUCCESS)
}

/// Fails when the committed query cache does not match the migrations and queries.
pub(crate) fn check(root: &Path) -> Result<ExitCode> {
    let fresh = tempfile::tempdir().context("cannot create a temporary directory")?;
    generate(root, fresh.path())?;
    let differences = compare(&read_dir(&query_cache(root))?, &read_dir(fresh.path())?);
    if differences.is_empty() {
        println!("query cache is up to date");
        return Ok(ExitCode::SUCCESS);
    }
    for difference in &differences {
        println!("{difference}");
    }
    println!("run `cargo xtask db prepare` and commit the result");
    Ok(ExitCode::FAILURE)
}

fn generate(root: &Path, output: &Path) -> Result<()> {
    let scratch = tempfile::tempdir().context("cannot create a temporary directory")?;
    let database = scratch.path().join("prepare.db");
    migrate(&root.join("crates/store/migrations"), &database)?;
    fs::create_dir_all(output).with_context(|| format!("cannot create {}", output.display()))?;

    let target = root.join("target/sqlx");
    let url = sqlite_url(&database);
    cargo(
        root,
        &["clean", "--package", STORE, "--target-dir"],
        &target,
        &[],
    )?;
    cargo(
        root,
        &["check", "--package", STORE, "--all-targets", "--target-dir"],
        &target,
        &[
            ("DATABASE_URL", url.as_str()),
            ("SQLX_OFFLINE", "false"),
            ("SQLX_OFFLINE_DIR", &output.display().to_string()),
        ],
    )
}

fn sqlite_url(path: &Path) -> String {
    format!("sqlite:{}", path.display().to_string().replace('\\', "/"))
}

fn migrate(migrations: &Path, database: &Path) -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("cannot start the async runtime")?;
    runtime.block_on(async {
        let options = SqliteConnectOptions::new()
            .filename(database)
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .context("cannot create the scratch database")?;
        Migrator::new(migrations)
            .await
            .context("cannot read the migrations")?
            .run(&pool)
            .await
            .context("cannot apply the migrations")?;
        pool.close().await;
        Ok(())
    })
}

fn cargo(root: &Path, args: &[&str], target: &Path, env: &[(&str, &str)]) -> Result<()> {
    let program = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let status = Command::new(program)
        .args(args)
        .arg(target)
        .envs(env.iter().copied())
        .current_dir(root)
        .status()
        .with_context(|| format!("failed to run cargo {}", args.join(" ")))?;
    if !status.success() {
        bail!("cargo {} failed", args.join(" "));
    }
    Ok(())
}

fn read_dir(dir: &Path) -> Result<BTreeMap<String, String>> {
    let mut files = BTreeMap::new();
    if !dir.exists() {
        return Ok(files);
    }
    for entry in fs::read_dir(dir).with_context(|| format!("cannot read {}", dir.display()))? {
        let path = entry?.path();
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .with_context(|| format!("unexpected file {}", path.display()))?
            .to_owned();
        let content =
            fs::read_to_string(&path).with_context(|| format!("cannot read {}", path.display()))?;
        files.insert(name, content);
    }
    Ok(files)
}

fn compare(committed: &BTreeMap<String, String>, fresh: &BTreeMap<String, String>) -> Vec<String> {
    let mut differences = Vec::new();
    for (name, content) in fresh {
        match committed.get(name) {
            None => differences.push(format!("missing {name}")),
            Some(old) if old != content => differences.push(format!("outdated {name}")),
            Some(_) => {}
        }
    }
    for name in committed.keys().filter(|name| !fresh.contains_key(*name)) {
        differences.push(format!("stale {name}"));
    }
    differences
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(name, content)| ((*name).to_owned(), (*content).to_owned()))
            .collect()
    }

    #[test]
    fn reports_missing_outdated_and_stale_entries() {
        let committed = files(&[("a.json", "1"), ("b.json", "1"), ("old.json", "1")]);
        let fresh = files(&[("a.json", "1"), ("b.json", "2"), ("new.json", "1")]);
        assert_eq!(
            compare(&committed, &fresh),
            vec!["outdated b.json", "missing new.json", "stale old.json"]
        );
    }

    #[test]
    fn builds_sqlite_urls_with_forward_slashes() {
        assert_eq!(
            sqlite_url(Path::new(r"C:\Users\Some One\prepare.db")),
            "sqlite:C:/Users/Some One/prepare.db"
        );
        assert_eq!(
            sqlite_url(Path::new("/tmp/prepare.db")),
            "sqlite:/tmp/prepare.db"
        );
    }

    #[test]
    fn equal_caches_have_no_differences() {
        let cache = files(&[("a.json", "1")]);
        assert_eq!(compare(&cache, &cache), Vec::<String>::new());
    }
}
