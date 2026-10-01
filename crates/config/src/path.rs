//! `config.rootify`: absolute paths stay put, relative paths join the home directory.
//!
//! Joining follows Go's `filepath.Join` / `filepath.Clean` on Unix (slash separators,
//! `.` and `..` removed, trailing slash dropped).

use std::path::PathBuf;

pub(crate) fn is_abs(path: &str) -> bool {
    path.starts_with('/')
}

pub(crate) fn rootify(path: &str, root: &str) -> PathBuf {
    if is_abs(path) {
        PathBuf::from(path)
    } else {
        PathBuf::from(go_join(root, path))
    }
}

pub(crate) fn go_join(a: &str, b: &str) -> String {
    if a.is_empty() && b.is_empty() {
        return String::new();
    }
    if a.is_empty() {
        return go_clean(b);
    }
    if b.is_empty() {
        return go_clean(a);
    }
    if is_abs(b) {
        return go_clean(b);
    }
    go_clean(&format!("{a}/{b}"))
}

fn go_clean(path: &str) -> String {
    if path.is_empty() {
        return ".".to_owned();
    }
    let abs = is_abs(path);
    let mut stack = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if stack.last().is_some_and(|prev| *prev != "..") {
                    stack.pop();
                } else if !abs {
                    stack.push("..");
                }
            }
            other => stack.push(other),
        }
    }
    if abs {
        if stack.is_empty() {
            "/".to_owned()
        } else {
            format!("/{}", stack.join("/"))
        }
    } else if stack.is_empty() {
        ".".to_owned()
    } else {
        stack.join("/")
    }
}

#[cfg(test)]
mod tests {
    use super::{go_join, rootify};

    #[test]
    fn rootify_matches_go_filepath() {
        assert_eq!(rootify("bar", "/foo").as_os_str(), "/foo/bar");
        assert_eq!(rootify("/opt/data", "/foo").as_os_str(), "/opt/data");
        assert_eq!(rootify("wal/mem/", "/foo").as_os_str(), "/foo/wal/mem");
        assert_eq!(go_join("config", "file.crt"), "config/file.crt");
        assert_eq!(go_join("config", ""), "config");
    }
}
