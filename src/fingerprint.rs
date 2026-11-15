//! Source identity, not a backup or proof about external services/environment.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path};
use std::process::{Command, Output};

use sha2::{Digest, Sha256};

use crate::store::Result;

/// Fixed, versioned scope: tracked/index entries plus non-ignored untracked
/// files, excluding state, dependencies, outputs and common secret files.
pub const SCOPE: &str = "git-source-v1";

#[derive(Debug, PartialEq, Eq, serde::Serialize)]
pub struct Snapshot {
    pub scope: &'static str,
    pub fingerprint: String,
}

/// Capture twice and reject observed changes. Call before AND after a check;
/// only matching snapshots may be attached to its result. This cannot detect
/// every transient edit: the caller must ensure a stable source tree.
pub fn capture(root: impl AsRef<Path>) -> Result<Snapshot> {
    let root = fs::canonicalize(root)?;
    let first = scan(&root)?;
    let second = scan(&root)?;
    if first != second {
        return Err("source changed during fingerprint capture".into());
    }
    Ok(Snapshot { scope: SCOPE, fingerprint: first })
}

/// Unknown scopes or incomplete captures never imply current evidence.
pub fn freshness(status: &str, scope: Option<&str>, verified: Option<&str>, current: Option<&Snapshot>) -> &'static str {
    if matches!(status, "not_run" | "not_required") {
        return "not_applicable";
    }
    match (scope, verified, current) {
        (Some(scope), Some(verified), Some(current)) if scope == current.scope => {
            if verified == current.fingerprint { "current" } else { "stale" }
        }
        _ => "unknown",
    }
}

fn scan(root: &Path) -> Result<String> {
    let top = git_ok(root, &["rev-parse", "--show-toplevel"])?;
    let top = std::str::from_utf8(&top)?.strip_suffix('\n').ok_or("invalid Git root")?;
    if fs::canonicalize(top)? != root {
        return Err("fingerprint root must be the Git working-tree root".into());
    }
    let mut hash = Sha256::new();
    field(&mut hash, SCOPE.as_bytes());
    let head = git(root, &["rev-parse", "--verify", "--quiet", "HEAD"])?;
    if head.status.success() {
        field(&mut hash, &head.stdout);
    } else if head.status.code() == Some(1) {
        field(&mut hash, b"unborn");
        field(&mut hash, &git_ok(root, &["symbolic-ref", "HEAD"])?);
    } else {
        return Err("cannot read Git HEAD".into());
    }

    // Keep index identity independently of working-file content so staging
    // changes are observable even when the working files stay identical.
    let mut entries: BTreeMap<String, Vec<Vec<u8>>> = BTreeMap::new();
    let index = git_ok(root, &["ls-files", "--stage", "-z"])?;
    for record in index.split(|byte| *byte == 0).filter(|record| !record.is_empty()) {
        let tab = record.iter().position(|byte| *byte == b'\t').ok_or("invalid Git index record")?;
        let path = std::str::from_utf8(&record[tab + 1..])?;
        validate_path(path)?;
        if excluded(path) { continue; }
        if record.starts_with(b"160000 ") {
            return Err("submodule content is not supported by this fingerprint scope".into());
        }
        entries.entry(path.to_owned()).or_default().push(record[..tab].to_vec());
    }
    let others = git_ok(root, &["ls-files", "--others", "--exclude-standard", "-z"])?;
    for path in others.split(|byte| *byte == 0).filter(|path| !path.is_empty()) {
        let path = std::str::from_utf8(path)?;
        validate_path(path)?;
        if !excluded(path) {
            entries.entry(path.to_owned()).or_default();
        }
    }
    for (path, index_records) in entries {
        field(&mut hash, path.as_bytes());
        field(&mut hash, &(index_records.len() as u64).to_be_bytes());
        for record in &index_records { field(&mut hash, record); }
        // Refuse symlink ancestors rather than read outside the project root.
        let relative = Path::new(&path);
        let mut parent = root.to_path_buf();
        for component in relative.parent().into_iter().flat_map(Path::components) {
            parent.push(component);
            match fs::symlink_metadata(&parent) {
                Ok(meta) if meta.file_type().is_dir() => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
                _ => return Err("unsupported source path ancestor".into()),
            }
        }
        let full_path = root.join(relative);
        let meta = match fs::symlink_metadata(&full_path) {
            Ok(meta) => meta,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && !index_records.is_empty() => {
                field(&mut hash, b"deleted");
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        // Conservative v1: symlinks/special files are unknown, not partial hashes.
        if !meta.file_type().is_file() {
            return Err("symlinks and non-regular source files are unsupported".into());
        }
        field(&mut hash, b"file");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            field(&mut hash, &(meta.permissions().mode() & 0o111).to_be_bytes());
        }
        #[cfg(not(unix))]
        field(&mut hash, &[u8::from(meta.permissions().readonly())]);
        let mut file = File::open(&full_path)?;
        let mut content = Sha256::new();
        let mut buffer = [0u8; 65536];
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 { break; }
            content.update(&buffer[..count]);
        }
        field(&mut hash, &content.finalize());
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn field(hash: &mut Sha256, value: &[u8]) {
    hash.update((value.len() as u64).to_be_bytes());
    hash.update(value);
}

fn validate_path(path: &str) -> Result<()> {
    if path.is_empty() || !Path::new(path).components().all(|part| matches!(part, Component::Normal(_))) {
        return Err("invalid source path".into());
    }
    Ok(())
}

fn excluded(path: &str) -> bool {
    path.split('/').any(|part| {
        matches!(part, ".git" | ".devstate" | "node_modules" | "target" | "dist" |
            "build" | ".next" | ".venv" | "vendor" | "secrets" | ".env")
            || part.starts_with(".env.") || part.ends_with(".pem") || part.ends_with(".key")
    })
}

fn git(root: &Path, args: &[&str]) -> Result<Output> {
    let mut command = Command::new("git");
    command.current_dir(root).args(args)
        .env("GIT_OPTIONAL_LOCKS", "0").env("GIT_NO_LAZY_FETCH", "1");
    for key in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY", "GIT_ALTERNATE_OBJECT_DIRECTORIES"] {
        command.env_remove(key);
    }
    Ok(command.output()?)
}

fn git_ok(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = git(root, args)?;
    if !output.status.success() {
        return Err("Git could not enumerate complete source state".into());
    }
    Ok(output.stdout)
}
