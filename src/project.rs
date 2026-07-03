//! The project file index — a bounded, session-cached list of files under the
//! project root, feeding the `@` file picker. Lazy (built on first use),
//! refreshable. v1 uses a skip-list (not `.gitignore`); the `ignore` crate is
//! the documented later upgrade for full gitignore fidelity.

use std::path::{Path, PathBuf};

pub struct Index {
    pub root: PathBuf,
    /// Paths relative to `root`, forward-slashed, for display + fuzzy matching.
    pub files: Vec<String>,
}

/// Nearest ancestor of `start` containing `.git`, else `start` itself.
pub fn project_root(start: &Path) -> PathBuf {
    let mut dir = start;
    loop {
        if dir.join(".git").exists() {
            return dir.to_path_buf();
        }
        match dir.parent() {
            Some(p) => dir = p,
            None => return start.to_path_buf(),
        }
    }
}

impl Index {
    /// Walk `root`, skipping `ignore` dirs and dotdirs, capped at `max` files.
    pub fn build(root: PathBuf, max: usize, ignore: &[String]) -> Index {
        let mut files = Vec::new();
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            if files.len() >= max {
                break;
            }
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                if is_dir {
                    // Skip dotdirs and the ignore-list.
                    if name.starts_with('.') || ignore.iter().any(|i| i == &name) {
                        continue;
                    }
                    stack.push(path);
                } else {
                    if files.len() >= max {
                        break;
                    }
                    if let Ok(rel) = path.strip_prefix(&root) {
                        files.push(rel.to_string_lossy().replace('\\', "/"));
                    }
                }
            }
        }
        files.sort();
        Index { root, files }
    }
}
