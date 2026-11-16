use std::path::PathBuf;

/// Where RMI keeps its files. `--portable` puts everything in `./data/` next to the exe.
#[derive(Debug, Clone)]
pub struct AppPaths {
    pub root: PathBuf,
    pub backups: PathBuf,
    pub logs: PathBuf,
    pub cache: PathBuf,
    /// Fallbacks and directory-creation failures hit while resolving. The logger does not
    /// exist yet, so `resolve` also eprintln!s them and `run` logs them from `.setup()`.
    pub problems: Vec<String>,
}

/// Debug builds only: `RMI_DATA_DIR` runs RMI as an isolated instance in that dir (E2E).
/// Release builds never read it.
pub fn override_dir() -> Option<PathBuf> {
    cfg!(debug_assertions)
        .then(|| std::env::var_os("RMI_DATA_DIR"))
        .flatten()
        .map(PathBuf::from)
}

impl AppPaths {
    pub fn resolve(portable: bool) -> Self {
        let mut problems = Vec::new();
        let root = if portable {
            match std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|d| d.join("data")))
            {
                Some(r) => r,
                None => {
                    problems.push("cannot find the exe path; using ./data".to_owned());
                    PathBuf::from("data")
                }
            }
        } else {
            match dirs::data_dir() {
                Some(d) => d.join("Roblox Multi Instance"),
                None => {
                    problems.push(
                        "cannot find the user data dir; using ./Roblox Multi Instance".to_owned(),
                    );
                    PathBuf::from("Roblox Multi Instance")
                }
            }
        };
        let mut p = Self::at(root);
        problems.append(&mut p.problems);
        for m in &problems {
            eprintln!("RMI: {m}");
        }
        p.problems = problems;
        p
    }

    pub fn at(root: PathBuf) -> Self {
        let mut p = Self {
            backups: root.join("backups"),
            logs: root.join("logs"),
            cache: root.join("cache"),
            root,
            problems: Vec::new(),
        };
        for d in [&p.root, &p.backups, &p.logs, &p.cache] {
            if let Err(e) = std::fs::create_dir_all(d) {
                p.problems
                    .push(format!("could not create {}: {e}", d.display()));
            }
        }
        p
    }

    pub fn settings(&self) -> PathBuf {
        self.root.join("settings.json")
    }

    pub fn vault(&self) -> PathBuf {
        self.root.join("vault.rmiv")
    }

    pub fn dk_cache(&self) -> PathBuf {
        self.root.join("dk.cache")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_create_dir_failure() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("f");
        std::fs::write(&file, "x").unwrap();
        // a file where the root dir should be
        let p = AppPaths::at(file.join("root"));
        assert!(!p.problems.is_empty());
        assert!(AppPaths::at(d.path().join("ok")).problems.is_empty());
    }
}
