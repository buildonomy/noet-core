use serde::{Deserialize, Serialize};
use std::{
    borrow::Cow,
    fmt::{Display, Formatter},
    path::{Component, Path, PathBuf, MAIN_SEPARATOR_STR},
};

/// Utility function to replace separators and convert to unicode (via to_string_lossy) on os path.
pub fn os_path_to_string<P: AsRef<Path>>(os_path_ref: P) -> String {
    let res = os_path_ref
        .as_ref()
        .components()
        .map(|c| match c {
            Component::RootDir => Cow::from("".to_string()),
            _ => c.as_os_str().to_string_lossy(),
        })
        .collect::<Vec<_>>()
        .join("/");
    tracing::debug!(
        "os_path_to_string: turned {:?} into {}",
        os_path_ref.as_ref().components(),
        res
    );
    res
}

pub fn string_to_os_path(path_string: &str) -> PathBuf {
    let res = PathBuf::from(path_string.replace("/", MAIN_SEPARATOR_STR));
    tracing::debug!("string_to_os_path: turned '{}' into {:?}", path_string, res);
    res
}

/// Turn a title string into a regularized anchor string
pub fn to_anchor(title: &str) -> String {
    title
        .trim()
        .to_lowercase()
        .replace(char::is_whitespace, "-")
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '-')
        .collect()
}

pub fn as_anchor(anchor: &str) -> String {
    let anchorized = to_anchor(anchor);
    if !anchorized.is_empty() {
        format!("#{anchorized}")
    } else {
        "".to_string()
    }
}

/// WASM-compatible path context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnchorPath<'a> {
    pub path: &'a str,
    /// Index of '/' separating path from file,
    dir_sep: Option<usize>,
    /// Index of '.' separating filename from extension
    ext_sep: Option<usize>,
    /// Index of '.' separating filename from extension
    anc_sep: Option<usize>,
}

/// Split a URL path into a (dir_path, filename, anchor) tuple
///
/// See tests module for examples.
///
impl<'a> AnchorPath<'a> {
    pub fn new(path: &'a str) -> AnchorPath<'a> {
        let anc_sep = path.find('#');

        let fullpath_range = 0..anc_sep.unwrap_or(path.len());
        let mut dir_sep = path[fullpath_range].rfind('/');
        let filename_range = dir_sep.map(|sep| sep + 1).unwrap_or(0)..anc_sep.unwrap_or(path.len());
        let mut ext_sep = path[filename_range].rfind('.');
        if let Some(dot_idx) = ext_sep {
            // Don't count hidden paths as extension markers
            if dot_idx == 0 {
                ext_sep = None;
            } else {
                // Make the index self relative
                ext_sep = Some(dot_idx + dir_sep.map(|sep| sep + 1).unwrap_or(0));
            }
        }
        // top is a dir, not a file
        if ext_sep.is_none()
            && dir_sep.is_some()
            && !path[dir_sep.map(|idx| idx + 1).unwrap()..anc_sep.unwrap_or(path.len())].is_empty()
        {
            dir_sep = anc_sep;
        }

        AnchorPath {
            path,
            dir_sep,
            ext_sep,
            anc_sep,
        }
    }

    pub fn is_absolute(&self) -> bool {
        self.dir().starts_with('/')
    }

    pub fn is_anchor(&self) -> bool {
        self.anc_sep.filter(|idx| *idx == 0).is_some()
    }

    pub fn dir(&self) -> &'a str {
        // Only capture leading slash, not trailing slash
        let stop_idx = self
            .dir_sep
            .map(|idx| if self.path.len() == 1 { idx + 1 } else { idx })
            .unwrap_or_else(|| {
                if self.ext_sep.is_some() {
                    0
                } else {
                    self.anc_sep.unwrap_or(self.path.len())
                }
            });
        &self.path[0..stop_idx]
    }

    pub fn anchor(&self) -> &'a str {
        let start_idx = self.anc_sep.map(|idx| idx + 1).unwrap_or(self.path.len());
        &self.path[start_idx..self.path.len()]
    }

    pub fn filename(&self) -> &'a str {
        let start_idx = self.dir_sep.map(|idx| idx + 1).unwrap_or(0);
        let stop_idx = if self.ext_sep.is_none() {
            start_idx
        } else {
            self.anc_sep.unwrap_or(self.path.len())
        };
        &self.path[start_idx..stop_idx]
    }

    pub fn filestem(&self) -> &'a str {
        let start_idx = self.dir_sep.map(|idx| idx + 1).unwrap_or(0);
        let stop_idx = self.ext_sep.unwrap_or(start_idx);
        &self.path[start_idx..stop_idx]
    }

    pub fn parent(&self) -> &'a str {
        if self.anc_sep.is_some() {
            self.filepath()
        } else if self.ext_sep.is_some() {
            self.dir()
        } else {
            let dir_sep = self.dir_sep.unwrap_or(self.path.len());
            let next_sep = self.path[0..dir_sep].rfind('/');
            &self.path[0..next_sep.unwrap_or(0)]
        }
    }

    /// Get the file extension from a URL path (anchor-aware)
    ///
    /// Strips any anchor fragment before extracting the extension.
    ///
    /// See tests module for examples.
    pub fn ext(&self) -> &'a str {
        let stop_idx = self.anc_sep.unwrap_or(self.path.len());
        let start_idx = self.ext_sep.map(|idx| idx + 1).unwrap_or(stop_idx);
        &self.path[start_idx..stop_idx]
    }

    pub fn filepath(&self) -> &'a str {
        if self.ext_sep.is_some() {
            &self.path[0..self.anc_sep.unwrap_or(self.path.len())]
        } else {
            self.dir()
        }
    }

    /// Calculate the join between two AnchorPaths.
    /// See tests module for examples.
    pub fn join<E: AsRef<str>>(&self, end_ref: E) -> String {
        let end = AnchorPath::from(end_ref.as_ref());
        if end.is_absolute() {
            return end.to_string();
        }
        if end.path.is_empty() {
            return self.to_string();
        }
        let mut pieces = Vec::<&str>::default();
        if !self.dir().is_empty() {
            pieces.push(self.dir());
        }
        if !end.dir().is_empty() {
            pieces.push(end.dir());
        }
        if !end.filename().is_empty() {
            pieces.push(end.filename());
        }
        if end.filepath().is_empty() && !end.anchor().is_empty() && !self.filename().is_empty() {
            pieces.push(self.filename());
        }
        let filepath = AnchorPath::new(&pieces.join("/")).normalize();
        let res = format!("{}{}", filepath, as_anchor(end.anchor()));
        res
    }

    /// Normalize a URL path by resolving `.` and `..` components
    ///
    /// Preserves leading `..` components (standard path normalization behavior).
    /// Callers should check the result if they need to validate against backtracking.
    ///
    /// See tests module for examples.
    pub fn normalize(&self) -> String {
        let mut components = Vec::new();
        let mut pop_dist = 0;
        for part in self.filepath().split('/') {
            match part {
                "" => {
                    if components.is_empty() {
                        // Preserve absolute paths
                        components.push("")
                    }
                    // Otherwise skip
                }
                "." => {
                    // Skip current dir references
                }
                ".." => {
                    // Try to go up one level
                    pop_dist += 1;
                }
                _ => {
                    let mut push_part = true;
                    let pop_diff = if pop_dist > components.len() {
                        pop_dist - components.len()
                    } else {
                        0
                    };
                    if pop_diff > 0 {
                        components = vec![".."; pop_diff];
                        pop_dist = 0;
                    } else if pop_dist > 0 {
                        let idx = components.len() - pop_dist;
                        let keep_part = part == components[idx];
                        if keep_part {
                            push_part = false;
                            pop_dist -= 1;
                        } else {
                            for _ in 0..pop_dist {
                                components.pop();
                            }
                            pop_dist = 0;
                        }
                    }
                    if push_part {
                        components.push(part);
                    }
                }
            }
        }
        for _ in 0..pop_dist {
            components.pop();
        }

        let filepath = components.join("/");
        let res = if !self.anchor().is_empty() {
            format!("{}#{}", filepath, self.anchor())
        } else {
            filepath
        };
        res
    }

    /// Calculate relative path from source document to target document.
    ///
    /// * `to_ref` - Path to target document (e.g., "docs/reference/api.md")
    ///
    /// * rooted - Imperative command that self.path and to_ref share the same relative root,
    ///   whether or not is_absolute is true. Will remove checking of is_absolute, and joining of
    ///   to_ref onto self.filepath() before comparing the two paths and finding the path_to.
    ///
    /// # Returns
    ///
    /// Relative path from source to target with forward slashes (e.g., "reference/api.md").
    /// Path separators are always normalized to forward slashes for cross-platform
    /// Markdown/URL compatibility, regardless of the host OS.
    ///
    /// See tests module for examples.
    pub fn path_to<E: AsRef<str>>(&self, to_ref: E, rooted: bool) -> String {
        let normalized_from = if rooted && self.is_absolute() {
            AnchorPath::new(self.path.trim_start_matches('/')).normalize()
        } else {
            self.normalize()
        };
        let normalized_to = if rooted {
            AnchorPath::new(to_ref.as_ref().trim_start_matches('/')).normalize()
        } else {
            AnchorPath::new(to_ref.as_ref()).normalize()
        };
        let from_clean = AnchorPath::from(&normalized_from);
        let to_clean = AnchorPath::from(&normalized_to);

        // Check if to_path starts with anchor - handle same-document anchors
        if to_clean.path.starts_with('#') {
            return to_clean.to_string();
        }

        if to_clean.is_absolute() && !from_clean.is_absolute() {
            return to_clean.to_string();
        }

        let joined_string = if !rooted {
            from_clean.join(to_clean)
        } else {
            normalized_to
        };
        let joined = AnchorPath::from(&joined_string);

        // Check for same document with different anchors early
        if joined.filepath() == from_clean.filepath() {
            return as_anchor(joined.anchor());
        }

        if joined.dir() == from_clean.dir() {
            return format!("{}{}", joined.filename(), as_anchor(joined.anchor()));
        }

        // We know from_clean and joined are normalized, so the only situations where there can be
        // an empty string on this split are "".split("/"), "/rooted_path".split("/"), or
        // "/".split("/"). We also know that joined is_absolute, and/or relative to from_clean,
        // because that's how `fn join` works.
        let from_parts: Vec<&str> = from_clean
            .dir()
            .split('/')
            .filter(|part| !part.is_empty())
            .collect();
        let to_parts: Vec<&str> = joined
            .dir()
            .split('/')
            .filter(|part| !part.is_empty())
            .collect();
        // Find common prefix length
        let mut common_len = 0;
        for (from_part, to_part) in from_parts.iter().zip(to_parts.iter()) {
            if from_part == to_part && !from_part.is_empty() {
                common_len += 1;
            } else {
                break;
            }
        }
        // If both are rooted and don't share a nonempty path, return absolute to_path
        if common_len == 0 && from_clean.is_absolute() {
            return joined_string;
        }

        let from_backtrack_len = if rooted {
            to_parts.iter().filter(|part| **part == "..").count()
        } else {
            0
        };
        // Build relative path
        let mut result = Vec::new();

        // Add ../ for each remaining directory in from_path

        if (from_parts.len() - from_backtrack_len) > common_len {
            for _ in common_len..(from_parts.len() - from_backtrack_len) {
                result.push("..".to_string());
            }
        }
        // Add remaining parts of to_path
        for part in &to_parts[common_len..] {
            result.push(part.to_string());
        }

        format!(
            "{}{}{}{}",
            result.join("/"),
            if !joined.filename().is_empty() {
                "/"
            } else {
                ""
            },
            joined.filename(),
            as_anchor(joined.anchor())
        )
    }

    /// Turns prefix into an anchorpath, takes its ap.filepath(), attempts to strip that from
    /// self.path, then removes any leading slashes. If the routine fails, returns self.path
    pub fn strip_prefix(&self, prefix: &str) -> Option<&'a str> {
        let prefix_ap = AnchorPath::new(prefix);
        self.path
            .strip_prefix(prefix_ap.filepath())
            .map(|remainder| remainder.trim_start_matches('/'))
    }

    pub fn replace_extension(&self, new_extension: &str) -> String {
        if self.ext().is_empty() {
            return self.path.to_string();
        }
        let dot_ext = format!(".{}", self.ext());
        let new_ext = format!(".{}", new_extension);
        format!(
            "{}{}",
            self.filepath().replace(&dot_ext, &new_ext),
            as_anchor(self.anchor())
        )
    }
}

impl<'a, T: AsRef<str> + ?Sized> From<&'a T> for AnchorPath<'a> {
    fn from(s: &'a T) -> AnchorPath<'a> {
        AnchorPath::new(s.as_ref())
    }
}

impl<'a> AsRef<str> for AnchorPath<'a> {
    fn as_ref(&self) -> &str {
        self.path
    }
}

impl<'a> Display for AnchorPath<'a> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.path)
    }
}

#[cfg(test)]
mod tests {
    use crate::tests::helpers::init_logging;

    use super::*;

    #[test]
    fn test_anchor_path_parsing() {
        let pa = AnchorPath::from("dir/file.md");
        assert_eq!(pa.dir(), "dir");
        assert_eq!(pa.filename(), "file.md");
        assert_eq!(pa.anchor(), "");
        assert_eq!(pa.filepath(), "dir/file.md");

        let pa = AnchorPath::from("network/dir/file.md#anchor");
        assert_eq!(pa.dir(), "network/dir");
        assert_eq!(pa.filename(), "file.md");
        assert_eq!(pa.anchor(), "anchor");
        assert_eq!(pa.filepath(), "network/dir/file.md");

        let pa = AnchorPath::from("/rooted/dir/file.md#anchor");
        assert_eq!(pa.dir(), "/rooted/dir");
        assert_eq!(pa.filename(), "file.md");
        assert_eq!(pa.anchor(), "anchor");
        assert_eq!(pa.filepath(), "/rooted/dir/file.md");

        // This shouldn't be allowed, but we need to define how we handle it
        let pa = AnchorPath::from("/rooted/dir/file.md#anchor/more-anchor");
        assert_eq!(pa.dir(), "/rooted/dir");
        assert_eq!(pa.filename(), "file.md");
        assert_eq!(pa.anchor(), "anchor/more-anchor");
        assert_eq!(pa.filepath(), "/rooted/dir/file.md");

        let pa = AnchorPath::from("dir");
        assert_eq!(pa.dir(), "dir");
        assert_eq!(pa.filename(), "");
        assert_eq!(pa.anchor(), "");
        assert_eq!(pa.filepath(), "dir");

        let pa = AnchorPath::from("network/dir/");
        assert_eq!(pa.dir(), "network/dir");
        assert_eq!(pa.filename(), "");
        assert_eq!(pa.anchor(), "");
        assert_eq!(pa.filepath(), "network/dir");

        let pa = AnchorPath::from("file.md");
        assert_eq!(pa.dir(), "");
        assert_eq!(pa.filename(), "file.md");
        assert_eq!(pa.anchor(), "");
        assert_eq!(pa.filepath(), "file.md");

        let pa = AnchorPath::from("file.md#anchor");
        assert_eq!(pa.dir(), "");
        assert_eq!(pa.filename(), "file.md");
        assert_eq!(pa.anchor(), "anchor");
        assert_eq!(pa.filepath(), "file.md");

        let pa = AnchorPath::from("");
        assert_eq!(pa.dir(), "");
        assert_eq!(pa.filename(), "");
        assert_eq!(pa.anchor(), "");
        assert_eq!(pa.filepath(), "");

        let pa = AnchorPath::from("/");
        assert_eq!(pa.dir(), "/");
        assert_eq!(pa.filename(), "");
        assert_eq!(pa.anchor(), "");
        assert_eq!(pa.filepath(), "/");

        let pa = AnchorPath::from("#anchor");
        assert_eq!(pa.dir(), "");
        assert_eq!(pa.filename(), "");
        assert_eq!(pa.anchor(), "anchor");
        assert_eq!(pa.filepath(), "");

        let pa = AnchorPath::from("network/dir#anchor");
        assert_eq!(pa.dir(), "network/dir");
        assert_eq!(pa.filename(), "");
        assert_eq!(pa.anchor(), "anchor");
        assert_eq!(pa.filepath(), "network/dir");
    }

    #[test]
    fn test_ext() {
        assert_eq!(AnchorPath::new("file.md").ext(), "md");
        assert_eq!(AnchorPath::new("file.md#anchor").ext(), "md");
        assert_eq!(AnchorPath::new("dir/file.html").ext(), "html");
        assert_eq!(AnchorPath::new("dir/file.html#section").ext(), "html");
        assert_eq!(AnchorPath::new("noextension").ext(), "");
        assert_eq!(AnchorPath::new("noextension/").ext(), "");
        assert_eq!(AnchorPath::new(".hidden_dir").ext(), "");
        assert_eq!(AnchorPath::new("/rooted/.hidden_dir").ext(), "");
        let net_hidden_dir_ap = AnchorPath::new("network/.hidden_dir");
        assert_eq!(net_hidden_dir_ap.ext(), "");
        assert_eq!(net_hidden_dir_ap.dir(), "network/.hidden_dir");
        assert_eq!(net_hidden_dir_ap.filename(), "");
        assert_eq!(net_hidden_dir_ap.filepath(), "network/.hidden_dir");
        assert_eq!(net_hidden_dir_ap.parent(), "network");
        assert_eq!(AnchorPath::new(".hidden_file.pdf").ext(), "pdf");
        assert_eq!(AnchorPath::new("noextension#anchor").ext(), "");
    }

    #[test]
    fn test_join() {
        // Relative path joining
        let ap = AnchorPath::from("docs/guide.md");
        assert_eq!(ap.dir(), "docs");
        assert_eq!(ap.filestem(), "guide");
        assert_eq!(ap.filepath(), "docs/guide.md");
        assert_eq!(ap.parent(), "docs");
        assert_eq!(&ap.join("api.md"), "docs/api.md");
        assert_eq!(
            AnchorPath::new("docs/guide.md").join("ref/types.md"),
            "docs/ref/types.md"
        );

        // Anchor joining (replaces anchor, doesn't join as path)
        assert_eq!(
            AnchorPath::from("docs/guide.md").join("#section"),
            "docs/guide.md#section"
        );
        assert_eq!(
            AnchorPath::from("docs/guide.md#old").join("#new"),
            "docs/guide.md#new"
        );

        // Empty string joining
        assert_eq!(AnchorPath::from("docs/guide.md").join(""), "docs/guide.md");

        // Absolute path joining (replaces base path)
        assert_eq!(
            AnchorPath::from("docs/guide.md").join("/other/path.md"),
            "/other/path.md"
        );
        assert_eq!(
            AnchorPath::from("/rooted/start").join("/rooted/file.md"),
            "/rooted/file.md"
        );
    }

    #[test]
    fn test_normalize() {
        assert_eq!(AnchorPath::from("dir/./file.md").normalize(), "dir/file.md");
        assert_eq!(
            AnchorPath::from("dir/sub/../file.md").normalize(),
            "dir/file.md"
        );
        assert_eq!(AnchorPath::from("../file.md").normalize(), "../file.md"); // Preserved
        assert_eq!(
            AnchorPath::from("../../dir/file.md").normalize(),
            "../../dir/file.md"
        );
        assert_eq!(
            AnchorPath::from("/dir/.//file.md").normalize(),
            "/dir/file.md"
        );
    }

    #[test]
    fn test_path_to() {
        init_logging();
        // relative to docs, move down a directory, then move back into docs directory
        let rel = AnchorPath::from("docs/guide.md").path_to("../docs/reference/api.md", false);
        assert_eq!(rel, "reference/api.md");

        let rel = AnchorPath::from("docs/guide.md").path_to("../docs/reference/api.md", true);
        assert_eq!(rel, "../docs/reference/api.md");
        // relative to docs, move down two directories, then move back into a

        // different docs directory
        let rel = AnchorPath::from("docs/guide.md").path_to("../../docs/reference/api.md", false);
        assert_eq!(rel, "../../docs/reference/api.md");

        // relative to docs, move down two directories, then move back into the same directory
        let rel = AnchorPath::from("docs/reference/guide.md")
            .path_to("../../docs/reference/api.md", false);
        assert_eq!(rel, "api.md");

        // relative to docs, move down two directories, then move back into the same directory
        let rel = AnchorPath::from("docs/reference/guide.md")
            .path_to("../../docs/../docs/reference/api.md", false);
        assert_eq!(rel, "api.md");

        // relative to docs, move down two directories, then move back into the same directory
        let rel = AnchorPath::from("docs/reference/guide.md")
            .path_to("../reference/../reference/api.md", false);
        assert_eq!(rel, "api.md");

        // rooted, this is, move down two directories, then access api.md
        let rel = AnchorPath::from("docs/reference/guide.md").path_to("api.md", true);
        assert_eq!(rel, "../../api.md");

        // relative to docs, move down two directories, then move back into the same directory
        let rel =
            AnchorPath::from("/reference/../reference/guide.md").path_to("docs/api.md", false);
        assert_eq!(rel, "docs/api.md");

        let rel = AnchorPath::from("/reference/../reference/guide.md").path_to("docs/api.md", true);
        assert_eq!(rel, "../docs/api.md");

        // If rooted, we ignore the leading slash
        let rel = AnchorPath::from("reference/../reference/guide.md").path_to("/docs/api.md", true);
        assert_eq!(rel, "../docs/api.md");

        // relative to docs, move down a directory, then move back into a different directory
        let rel = AnchorPath::from("docs/guide.md").path_to("../tests/reference/api.md", false);
        assert_eq!(rel, "../tests/reference/api.md");

        let rel = AnchorPath::from("guide.md").path_to("reference/api.md", false);
        assert_eq!(rel, "reference/api.md");

        // if rooted, it's the same
        let rel = AnchorPath::from("guide.md").path_to("reference/api.md", true);
        assert_eq!(rel, "reference/api.md");

        // relation is relative to from_path directory, not any shared prefix. Test 1
        let rel = AnchorPath::from("docs/reference/types.md#about").path_to("docs/guide.md", false);
        assert_eq!(rel, "docs/guide.md");

        // relation is relative to from_path directory, not any shared prefix. Test 2
        let rel = AnchorPath::from("subnet1").path_to("subnet1_file1.md", false);
        assert_eq!(rel, "subnet1_file1.md");

        let rel = AnchorPath::from("docs/reference/.hidden_dir")
            .path_to("../.hidden_dir/guide.md", false);
        assert_eq!(rel, "guide.md");

        // # Rooted path tests
        // from_path is rooted dir and to_path is relative: just provide to_path
        let rel = AnchorPath::from("/rooted/path/").path_to("docs/reference/api.md", false);
        assert_eq!(rel, "docs/reference/api.md");

        // from_path is rooted file and to_path is relative: just provide to_path
        let rel = AnchorPath::from("/rooted/path/guide.md").path_to("docs/reference/api.md", false);
        assert_eq!(rel, "docs/reference/api.md");

        // from_path is not rooted and to_path is rooted - just provide to_path
        let rel = AnchorPath::from("docs/guide.md").path_to("/rooted/reference/api.md", false);
        assert_eq!(rel, "/rooted/reference/api.md");

        // from and to are rooted and share a common ancestor - provide shortest relative path
        let rel = AnchorPath::from("/rooted/path").path_to("/rooted/other/path/api.md", false);
        assert_eq!(rel, "../other/path/api.md");

        // from and to are rooted and only share root ancestor - just provide rooted to_path
        let rel = AnchorPath::from("/original/path").path_to("/other/rooted/path/api.md", false);
        assert_eq!(rel, "/other/rooted/path/api.md");

        // joining an anchor to a non-extensioned final path element works
        let rel = AnchorPath::from("/rooted/path").path_to("#anchor", false);
        assert_eq!(rel, "#anchor");

        // can join anchors to directories (in noet-core this enables us to get to anchors within
        // BeliefNetwork files)
        let rel = AnchorPath::from("/rooted/.path").path_to("#anchor", false);
        assert_eq!(rel, "#anchor");

        // can join an anchor to a hidden file.
        let rel = AnchorPath::from("relative/.path.md").path_to("#anchor", false);
        assert_eq!(rel, "#anchor");

        // can join an anchor to a hidden file.
        let rel = AnchorPath::from("relative/.path.md").path_to("#anchor", true);
        assert_eq!(rel, "#anchor");

        let rel =
            AnchorPath::from("reference/api.md#section-2").path_to("design.md#references", false);
        assert_eq!(rel, "design.md#references");

        let rel = AnchorPath::from("reference/design.md#section-2")
            .path_to("design.md#references", false);
        assert_eq!(rel, "#references");
    }

    #[test]
    fn test_strip_prefix() {
        assert_eq!(
            AnchorPath::from("dir/file.md").strip_prefix("dir"),
            Some("file.md")
        );
        assert_eq!(
            AnchorPath::from("dir/sub/file.md").strip_prefix("dir"),
            Some("sub/file.md")
        );
        assert_eq!(
            AnchorPath::from("dir/file.md#anchor").strip_prefix("dir/file.md#foo-bar"),
            Some("#anchor")
        );
        assert_eq!(
            AnchorPath::from("../file.md").strip_prefix(".."),
            Some("file.md")
        );
        assert_eq!(AnchorPath::from("file.md").strip_prefix("dir"), None);
        assert_eq!(
            AnchorPath::from("dir/file.md").strip_prefix("dir/"),
            Some("file.md")
        );
    }

    #[test]
    fn test_to_anchor_consistency() {
        // Verify to_anchor() behavior for collision detection
        assert_eq!(to_anchor("Details"), "details");
        assert_eq!(to_anchor("Section One"), "section-one");
        assert_eq!(to_anchor("API & Reference"), "api--reference");
        assert_eq!(to_anchor("My-Section!"), "my-section");

        // Case insensitivity
        assert_eq!(to_anchor("Details"), to_anchor("DETAILS"));
        assert_eq!(to_anchor("Section"), to_anchor("section"));
    }
}
