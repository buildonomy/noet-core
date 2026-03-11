use serde::{Deserialize, Serialize};
use std::{
    borrow::Cow,
    fmt::{Display, Formatter},
    ops::Deref,
    path::{Component, Path, PathBuf, MAIN_SEPARATOR_STR},
};
use unicode_normalization::UnicodeNormalization;

/// Utility function to replace separators and convert to unicode (via to_string_lossy) on os path.
/// On Windows, strips the `\\?\` UNC prefix but preserves drive letters for canonical path representation.
pub fn os_path_to_string<P: AsRef<Path>>(os_path_ref: P) -> String {
    #[cfg(windows)]
    use std::path::Prefix;

    // On Windows, a drive-letter path (e.g. `C:\Users\foo`) decomposes into:
    //   Prefix(Disk('C'))  →  "C:"
    //   RootDir            →  ""   ← if we emit this, join("/") gives "C://Users"
    //   Normal("Users")    →  "Users"
    //
    // The double slash would make AnchorPath treat "C:" as a URL schema — now moot
    // since AnchorPath is drive-letter-aware, but the single-slash form is still
    // the correct canonical representation and avoids any future ambiguity.
    //
    // Fix: track whether we just emitted a drive-letter Prefix and, if so, skip the
    // immediately following RootDir component (the backslash is implied by "C:").
    //
    // On Linux there is never a Prefix, so RootDir still emits "" and the join
    // produces the correct leading "/" for absolute paths.
    #[cfg(windows)]
    let mut skip_next_root_dir = false;

    let res = os_path_ref
        .as_ref()
        .components()
        .filter_map(|c| {
            match c {
                Component::RootDir => {
                    #[cfg(windows)]
                    if skip_next_root_dir {
                        skip_next_root_dir = false;
                        return None;
                    }
                    Some(Cow::from("".to_string()))
                }
                #[cfg(windows)]
                Component::Prefix(prefix) => {
                    // Extract drive letter from prefix, skip \\?\ verbatim prefix
                    match prefix.kind() {
                        Prefix::VerbatimDisk(letter) | Prefix::Disk(letter) => {
                            // Convert drive letter (e.g., b'C') to "C:"
                            // Signal that the following RootDir is redundant.
                            skip_next_root_dir = true;
                            Some(Cow::from(format!("{}:", letter as char)))
                        }
                        _ => {
                            // For other prefix types (UNC, VerbatimUNC, etc.), include as-is
                            Some(prefix.as_os_str().to_string_lossy())
                        }
                    }
                }
                #[cfg(not(windows))]
                Component::Prefix(_) => None,
                Component::Normal(s) => Some(s.to_string_lossy()),
                Component::CurDir => Some(Cow::from(".")),
                Component::ParentDir => Some(Cow::from("..")),
            }
        })
        .collect::<Vec<_>>()
        .join("/");
    res
}

pub fn string_to_os_path(path_string: &str) -> PathBuf {
    PathBuf::from(path_string.replace("/", MAIN_SEPARATOR_STR))
}

/// Turn a title string into a regularized anchor string.
///
/// Rules:
/// - NFKC normalization: resolves Unicode compatibility variants (`ﬁ`→`fi`,
///   `²`→`2`, `ℕ`→`N`, full-width chars→ASCII equivalents) without stripping
///   accents or destroying non-Latin scripts. This ensures that precomposed
///   and decomposed forms of the same character (e.g. `é` U+00E9 vs `e` +
///   combining accent) compare equal, while keeping genuinely distinct titles
///   like `Résumé` and `Resume` as distinct anchors.
/// - Lowercase
/// - ASCII whitespace → `-`
/// - Keep: alphanumeric (all Unicode scripts), `-`, `.`, `_`, `(`, `)`,
///   `[`, `]`, `@`. These are all valid in HTML5 `id=` attributes
///   (no-whitespace-only rule) and are legal in RFC 3986 URL fragments
///   (unreserved + sub-delims). Preserving them makes JS API titles like
///   `Temporal.Duration()`, `Array.prototype.map()`, `Symbol.iterator`,
///   and `for...in` produce distinct, human-readable anchors.
///   Non-ASCII alphanumeric chars (accented letters, CJK, Arabic, …) are
///   kept as-is — they are valid in HTML5 `id=` and modern browsers handle
///   non-ASCII URL fragments correctly.
/// - Strip everything else (e.g. `&`, `!`, `:`, `#`, `%`, `+`, `=`, …)
/// - Collapse runs of `-` to a single `-` and strip leading/trailing `-`
pub fn to_anchor(title: &str) -> String {
    // NFKC first: resolves compatibility variants without stripping accents
    let s: String = title.trim().nfkc().collect::<String>().to_lowercase();
    let s: String = s
        .chars()
        .map(|c| if c.is_ascii_whitespace() { '-' } else { c })
        .filter(|c| {
            c.is_alphanumeric() || matches!(c, '-' | '.' | '_' | '(' | ')' | '[' | ']' | '@')
        })
        .collect();
    // Collapse consecutive hyphens and strip leading/trailing hyphens
    let mut result = String::with_capacity(s.len());
    let mut prev_hyphen = false;
    for c in s.chars() {
        if c == '-' {
            if !prev_hyphen {
                result.push(c);
            }
            prev_hyphen = true;
        } else {
            result.push(c);
            prev_hyphen = false;
        }
    }
    // Strip leading/trailing hyphens
    result.trim_matches('-').to_string()
}

pub fn as_anchor(anchor: &str) -> String {
    let anchorized = to_anchor(anchor);
    if !anchorized.is_empty() {
        format!("#{anchorized}")
    } else {
        "".to_string()
    }
}

pub fn as_extension(ext: &str) -> String {
    if !ext.is_empty() {
        format!(".{ext}")
    } else {
        "".to_string()
    }
}

/// WASM-compatible path context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnchorPath<'a> {
    pub path: &'a str,
    /// Index of ':' separating schema from path
    sch_sep: Option<usize>,
    /// Index marking end of hostname (if present after schema://)
    host_sep: Option<usize>,
    /// Index of '?' separating path from query parameters
    param_sep: Option<usize>,
    /// Index of '/' separating path from file
    dir_sep: Option<usize>,
    /// Index of '.' separating filename from extension
    ext_sep: Option<usize>,
    /// Index of '#' separating path from anchor
    anc_sep: Option<usize>,
}

/// Split a URL path into a (dir_path, filename, anchor) tuple
///
/// See tests module for examples.
///
impl<'a> AnchorPath<'a> {
    pub fn new(path: &'a str) -> AnchorPath<'a> {
        // Parse in order: anchor (#), then params (?), then schema (:), then hostname
        let anc_sep = path.find('#');

        // Find param separator before anchor
        let param_search_range = 0..anc_sep.unwrap_or(path.len());
        let param_sep = path[param_search_range.clone()].rfind('?');

        // Find schema separator (: before any /, ?, or #)
        let sch_sep = if let Some(colon_idx) = path.find(':') {
            // Schema is valid only if ':' comes before first '/', '?', '#'
            let first_separator = [path.find('/'), param_sep, anc_sep]
                .iter()
                .filter_map(|&x| x)
                .min();

            // A single ASCII letter followed by ':' is a Windows drive letter (e.g. "C:"),
            // not a URL schema. Treat it as a plain absolute path so that filepath(), dir(),
            // join(), normalize(), etc. preserve the drive prefix rather than stripping it.
            let is_drive_letter = colon_idx == 1 && path.as_bytes()[0].is_ascii_alphabetic();

            if is_drive_letter {
                None
            } else if first_separator.is_none() || colon_idx < first_separator.unwrap() {
                Some(colon_idx)
            } else {
                None
            }
        } else {
            None
        };

        // Parse hostname only for hierarchical URLs (schema followed by //)
        // Non-hierarchical URLs like "mailto:user@example.com" or "data:text/plain,..."
        // don't have hostnames - everything after the schema is treated as the path
        let mut host_sep = None;
        let mut path_start = sch_sep.map(|idx| idx + 1).unwrap_or(0);

        if let Some(sch_idx) = sch_sep {
            let after_schema = sch_idx + 1;
            // Check if we have // after the schema (indicates hierarchical URL with authority).
            // Windows drive letters (e.g. "C:/...") are excluded before this block — they
            // produce sch_sep = None so we never reach here for them.
            // Zero-length schemas ("://path") and multi-character schemas (http, ftp, file, …)
            // have a real hostname authority; single-char non-alpha colons (edge case) do not.
            let schema_is_url = sch_idx != 1;
            if schema_is_url
                && after_schema + 1 < path.len()
                && path.as_bytes()[after_schema] == b'/'
                && path.as_bytes()[after_schema + 1] == b'/'
            {
                // This is a hierarchical URL - parse the hostname
                let host_start = after_schema + 2; // Skip the //
                                                   // Find end of hostname (next /, ?, or #)
                let host_end_range = host_start..param_sep.or(anc_sep).unwrap_or(path.len());
                let host_end = if let Some(slash_idx) = path[host_end_range.clone()].find('/') {
                    host_start + slash_idx
                } else {
                    param_sep.or(anc_sep).unwrap_or(path.len())
                };
                host_sep = Some(host_end);
                path_start = host_end;
            }
            // else: Non-hierarchical URL (no //), so no hostname parsing
            //       Examples: mailto:user@host, data:text/plain, javascript:alert()
        }

        // Calculate path range (after hostname/schema, before params/anchor)
        let path_end = param_sep.or(anc_sep).unwrap_or(path.len());

        let fullpath_range = path_start..path_end;
        let mut dir_sep = path[fullpath_range.clone()]
            .rfind('/')
            .map(|idx| idx + path_start);

        let filename_range = dir_sep.map(|sep| sep + 1).unwrap_or(path_start)..path_end;
        let mut ext_sep = path[filename_range.clone()].rfind('.');
        if let Some(dot_idx) = ext_sep {
            // Don't count hidden paths as extension markers
            if dot_idx == 0 {
                ext_sep = None;
            } else {
                // Make the index path relative
                ext_sep = Some(dot_idx + dir_sep.map(|sep| sep + 1).unwrap_or(path_start));
            }
        }
        // top is a dir, not a file
        if ext_sep.is_none()
            && dir_sep.is_some()
            && !path[dir_sep.map(|idx| idx + 1).unwrap_or(path_start)..path_end].is_empty()
        {
            dir_sep = param_sep.or(anc_sep);
        }

        AnchorPath {
            path,
            sch_sep,
            host_sep,
            param_sep,
            dir_sep,
            ext_sep,
            anc_sep,
        }
    }

    /// Construct an `AnchorPath` that treats the input as a **directory** path even when the
    /// last path component contains a dot. `AnchorPath::new` classifies dotted last components
    /// (e.g. `symbol.iterator`, `v1.2`) as files with an extension. This constructor undoes
    /// that classification so that:
    ///
    /// - `is_dir()` → `true`
    /// - `ext()` → `""`
    /// - `filestem()` → `""`
    /// - `filename()` → `""`
    /// - `dir()` → the full path (the dotted component becomes part of the directory)
    /// - `filepath()` → the full path (same as `dir()`)
    ///
    /// Use this whenever you have a filesystem path that is known to be a directory, so that
    /// a dotted directory name (e.g. `array/symbol.iterator`) is not misclassified as a file
    /// with extension `"iterator"` during codec lookup or path manipulation.
    ///
    /// Paths that `new` already treats as directories (no `ext_sep`) are returned unchanged.
    /// Trailing-slash paths and empty strings are also returned unchanged.
    pub fn new_dir(path: &'a str) -> AnchorPath<'a> {
        let mut ap = Self::new(path);
        if ap.ext_sep.is_some() {
            let path_end = ap.param_sep.or(ap.anc_sep).unwrap_or(path.len());
            // Only override when the path doesn't already end with '/' (bare root "/" is fine).
            if path_end > 0 && path.as_bytes().get(path_end - 1).copied() != Some(b'/') {
                // Clear ext_sep → is_dir() returns true, ext() and filestem() return "".
                ap.ext_sep = None;
                // Apply the "top is a dir" dir_sep promotion that new() skipped because it
                // saw an extension. This makes dir() return the full dotted-component path
                // and filename()/filepath() behave consistently with a bare directory.
                ap.dir_sep = ap.param_sep.or(ap.anc_sep);
            }
        }
        ap
    }

    /// Construct an `AnchorPath` that treats the input as a **file** path even when the
    /// filename has no extension. `AnchorPath::new` classifies extensionless paths as
    /// directories (`ext_sep = None`, `is_dir() == true`, `filestem() == ""`). This
    /// constructor forces `ext_sep` to point just past the last filename character so that:
    ///
    /// - `is_dir()` → `false`
    /// - `filestem()` → the full filename (`"Gemfile"`, `"Makefile"`, …)
    /// - `ext()` → `""` (no real extension; `ext()` is guarded against the out-of-range
    ///   sentinel value)
    /// - `filename()` → the full filename
    ///
    /// Use this whenever you have a filesystem path that is known to be a file (not a
    /// directory) and needs to be distinguishable from a bare directory path inside
    /// `AnchorPath` logic (e.g. codec lookup, `is_dir()` checks).
    ///
    /// Paths that already have an extension are returned unchanged (same as `new`).
    /// Trailing-slash paths and empty strings are also returned unchanged.
    pub fn new_file(path: &'a str) -> AnchorPath<'a> {
        let mut ap = Self::new(path);
        if ap.ext_sep.is_none() {
            let path_end = ap.param_sep.or(ap.anc_sep).unwrap_or(path.len());
            // Only override when there is an actual filename component (non-empty, not
            // a bare "/" or trailing slash).
            if path_end > 0 && path.as_bytes().get(path_end - 1).copied() != Some(b'/') {
                // Set ext_sep = path_end so that:
                //   filestem() stop_idx = path_end  → returns the whole filename
                //   ext() start_idx = path_end + 1  → clamped to "" by the guard in ext()
                ap.ext_sep = Some(path_end);
                // Undo the "top is a dir" dir_sep promotion that new() may have applied.
                let path_start = ap
                    .host_sep
                    .or_else(|| ap.sch_sep.map(|i| i + 1))
                    .unwrap_or(0);
                ap.dir_sep = path[path_start..path_end]
                    .rfind('/')
                    .map(|i| i + path_start);
            }
        }
        ap
    }

    pub fn is_absolute(&self) -> bool {
        // Standard Unix-style absolute path starts with '/'.
        // Windows absolute paths start with a drive letter followed by ':/' (e.g. "C:/").
        // Because drive letters are no longer parsed as URL schemas, filepath()/dir() now
        // include the "C:" prefix, so we must check both forms here.
        let d = self.dir();
        d.starts_with('/')
            || (d.len() >= 3
                && d.as_bytes()[0].is_ascii_alphabetic()
                && d.as_bytes()[1] == b':'
                && d.as_bytes()[2] == b'/')
    }

    pub fn is_anchor(&self) -> bool {
        self.anc_sep.filter(|idx| *idx == 0).is_some()
    }

    pub fn is_dir(&self) -> bool {
        self.ext_sep.is_none()
    }

    /// Check if this path has a URL schema (e.g., `http:`, `file:`, `https:`)
    ///
    /// # Examples
    /// ```
    /// use noet_core::paths::path::AnchorPath;
    ///
    /// let ap = AnchorPath::new("https://example.com/file.md");
    /// assert!(ap.has_schema());
    ///
    /// let ap = AnchorPath::new("dir/file.md");
    /// assert!(!ap.has_schema());
    /// ```
    pub fn has_schema(&self) -> bool {
        self.sch_sep.is_some()
    }

    /// Check if this path has a hostname (URL with schema://)
    ///
    /// # Examples
    /// ```
    /// use noet_core::paths::path::AnchorPath;
    ///
    /// let ap = AnchorPath::new("https://example.com/file.md");
    /// assert!(ap.has_hostname());
    ///
    /// let ap = AnchorPath::new("c:/Windows/file.txt");
    /// assert!(!ap.has_hostname());
    /// ```
    pub fn has_hostname(&self) -> bool {
        self.host_sep.is_some()
    }

    /// Check if this path has query parameters (e.g., `?page=2&sort=desc`)
    ///
    /// # Examples
    /// ```
    /// use noet_core::paths::path::AnchorPath;
    ///
    /// let ap = AnchorPath::new("file.md?page=2");
    /// assert!(ap.has_parameters());
    ///
    /// let ap = AnchorPath::new("file.md");
    /// assert!(!ap.has_parameters());
    /// ```
    pub fn has_parameters(&self) -> bool {
        self.param_sep.is_some()
    }

    /// Return the schema portion of the path (before the `:`)
    ///
    /// Returns an empty string if no schema is present.
    ///
    /// # Examples
    /// ```
    /// use noet_core::paths::path::AnchorPath;
    ///
    /// let ap = AnchorPath::new("https://example.com/file.md");
    /// assert_eq!(ap.schema(), "https");
    ///
    /// let ap = AnchorPath::new("file:///path/to/file");
    /// assert_eq!(ap.schema(), "file");
    ///
    /// let ap = AnchorPath::new("dir/file.md");
    /// assert_eq!(ap.schema(), "");
    /// ```
    pub fn schema(&self) -> &'a str {
        if let Some(idx) = self.sch_sep {
            &self.path[0..idx]
        } else {
            ""
        }
    }

    /// Return everything after the schema prefix, with leading slashes stripped.
    ///
    /// For hierarchical URLs (`schema://authority/path`), this returns the authority
    /// and path together with the `://` and any extra leading slashes removed.
    /// For non-hierarchical URLs (`schema:path`), returns the path after the colon.
    /// For bare paths (no schema), returns the full path unchanged.
    ///
    /// This is useful when the caller needs to do its own authority/path parsing
    /// (e.g., probing whether the first component is a known identifier).
    ///
    /// # Examples
    /// ```
    /// use noet_core::paths::path::AnchorPath;
    ///
    /// // Hierarchical URL — strips schema and ://
    /// let ap = AnchorPath::new("https://example.com/file.md");
    /// assert_eq!(ap.path_after_schema(), "example.com/file.md");
    ///
    /// // Extra slashes are stripped
    /// let ap = AnchorPath::new("bid://///some-value");
    /// assert_eq!(ap.path_after_schema(), "some-value");
    ///
    /// // Non-hierarchical URL
    /// let ap = AnchorPath::new("mailto:user@example.com");
    /// assert_eq!(ap.path_after_schema(), "user@example.com");
    ///
    /// // Bare path (no schema) — returns full path
    /// let ap = AnchorPath::new("dir/file.md#anchor");
    /// assert_eq!(ap.path_after_schema(), "dir/file.md#anchor");
    ///
    /// // Anchor-only
    /// let ap = AnchorPath::new("#section");
    /// assert_eq!(ap.path_after_schema(), "#section");
    /// ```
    pub fn path_after_schema(&self) -> &'a str {
        if let Some(sch_idx) = self.sch_sep {
            self.path[sch_idx + 1..].trim_start_matches('/')
        } else {
            self.path
        }
    }

    /// Return the resource portion of the URL — everything after the schema and hostname.
    ///
    /// This is the local path + query parameters + anchor fragment, i.e. the part of
    /// the URL that identifies a resource within the authority's namespace.
    ///
    /// For hierarchical URLs (`schema://host/path?params#anchor`), returns `/path?params#anchor`.
    /// For non-hierarchical URLs (`schema:path`), returns `path` (everything after `:`).
    /// For bare paths (no schema), returns the full path unchanged.
    ///
    /// Unlike [`filepath()`](AnchorPath::filepath), this method preserves query parameters
    /// and anchor fragments. Unlike [`path_after_schema()`](AnchorPath::path_after_schema),
    /// this method strips the hostname for hierarchical URLs.
    ///
    /// # Examples
    /// ```
    /// use noet_core::paths::path::AnchorPath;
    ///
    /// // Hierarchical URL — returns path + params + anchor after hostname
    /// let ap = AnchorPath::new("https://example.com/docs/file.md?page=2#section");
    /// assert_eq!(ap.resource(), "/docs/file.md?page=2#section");
    ///
    /// // Hierarchical URL — no params or anchor
    /// let ap = AnchorPath::new("https://example.com/docs/file.md");
    /// assert_eq!(ap.resource(), "/docs/file.md");
    ///
    /// // Hierarchical URL — hostname only, no path
    /// let ap = AnchorPath::new("https://example.com");
    /// assert_eq!(ap.resource(), "");
    ///
    /// // Hierarchical URL — empty hostname (e.g. file:///path)
    /// let ap = AnchorPath::new("file:///absolute/path.txt");
    /// assert_eq!(ap.resource(), "/absolute/path.txt");
    ///
    /// // Non-hierarchical URL — everything after schema:
    /// let ap = AnchorPath::new("mailto:user@example.com?subject=Hello");
    /// assert_eq!(ap.resource(), "user@example.com?subject=Hello");
    ///
    /// // Anchor-only
    /// let ap = AnchorPath::new("#section");
    /// assert_eq!(ap.resource(), "#section");
    ///
    /// // Bare path with anchor — returns full path
    /// let ap = AnchorPath::new("dir/file.md#section");
    /// assert_eq!(ap.resource(), "dir/file.md#section");
    ///
    /// // Custom scheme — hierarchical with anchor
    /// let ap = AnchorPath::new("id://network/my-id#sub");
    /// assert_eq!(ap.resource(), "/my-id#sub");
    /// ```
    pub fn resource(&self) -> &'a str {
        let start_idx = self
            .host_sep
            .or_else(|| self.sch_sep.map(|idx| idx + 1))
            .unwrap_or(0);
        &self.path[start_idx..]
    }

    /// Return the query parameters portion of the path (after `?`, before `#`)
    ///
    /// Returns an empty string if no parameters are present.
    ///
    /// # Examples
    /// ```
    /// use noet_core::paths::path::AnchorPath;
    ///
    /// let ap = AnchorPath::new("file.md?page=2&sort=desc");
    /// assert_eq!(ap.parameters(), "page=2&sort=desc");
    ///
    /// let ap = AnchorPath::new("file.md?id=123#section");
    /// assert_eq!(ap.parameters(), "id=123");
    ///
    /// let ap = AnchorPath::new("file.md");
    /// assert_eq!(ap.parameters(), "");
    /// ```
    pub fn parameters(&self) -> &'a str {
        if let Some(start_idx) = self.param_sep {
            let end_idx = self.anc_sep.unwrap_or(self.path.len());
            &self.path[start_idx + 1..end_idx]
        } else {
            ""
        }
    }

    /// Return the hostname portion of the path (after schema://)
    ///
    /// Returns an empty string if no hostname is present.
    ///
    /// # Examples
    /// ```
    /// use noet_core::paths::path::AnchorPath;
    ///
    /// let ap = AnchorPath::new("https://example.com/file.md");
    /// assert_eq!(ap.hostname(), "example.com");
    ///
    /// let ap = AnchorPath::new("https://user:pass@example.com:8080/path");
    /// assert_eq!(ap.hostname(), "user:pass@example.com:8080");
    ///
    /// let ap = AnchorPath::new("c:/Windows/file.txt");
    /// assert_eq!(ap.hostname(), "");
    /// ```
    pub fn hostname(&self) -> &'a str {
        if let (Some(sch_idx), Some(host_end)) = (self.sch_sep, self.host_sep) {
            // Start after schema and //
            let host_start = sch_idx + 3; // skip "://"
            &self.path[host_start..host_end]
        } else {
            ""
        }
    }

    pub fn dir(&self) -> &'a str {
        // Start after hostname if present, otherwise after schema
        let start_idx = self
            .host_sep
            .or_else(|| self.sch_sep.map(|idx| idx + 1))
            .unwrap_or(0);

        // Only capture leading slash, not trailing slash
        // Exception: if the path is just "/" (either absolute or after hostname), include it
        let stop_idx = self
            .dir_sep
            .map(|idx| {
                // Include the slash if it's the only character (like "/" or after "https://host/")
                if self.path.len() == 1
                    || (idx == start_idx
                        && idx + 1 == self.param_sep.or(self.anc_sep).unwrap_or(self.path.len()))
                {
                    idx + 1
                } else {
                    idx
                }
            })
            .unwrap_or_else(|| {
                if self.ext_sep.is_some() {
                    start_idx
                } else {
                    self.param_sep.or(self.anc_sep).unwrap_or(self.path.len())
                }
            });

        &self.path[start_idx..stop_idx]
    }

    pub fn anchor(&self) -> &'a str {
        let start_idx = self.anc_sep.map(|idx| idx + 1).unwrap_or(self.path.len());
        &self.path[start_idx..self.path.len()]
    }

    pub fn filename(&self) -> &'a str {
        let path_start = self
            .host_sep
            .or_else(|| self.sch_sep.map(|idx| idx + 1))
            .unwrap_or(0);
        let start_idx = self.dir_sep.map(|idx| idx + 1).unwrap_or(path_start);
        let stop_idx = if self.ext_sep.is_none() {
            start_idx
        } else {
            self.param_sep.or(self.anc_sep).unwrap_or(self.path.len())
        };
        &self.path[start_idx..stop_idx]
    }

    pub fn filestem(&self) -> &'a str {
        let path_start = self
            .host_sep
            .or_else(|| self.sch_sep.map(|idx| idx + 1))
            .unwrap_or(0);
        let start_idx = self.dir_sep.map(|idx| idx + 1).unwrap_or(path_start);
        let stop_idx = self.ext_sep.unwrap_or(start_idx);
        &self.path[start_idx..stop_idx]
    }

    /// Return the logical parent of this path as an [`AnchorPath<'a>`] (zero-allocation).
    ///
    /// - **File with no anchor** (`ext_sep.is_some()`, `anc_sep.is_none()`): returns the
    ///   containing directory via [`AnchorPath::new_dir`] so that a dotted directory name
    ///   (e.g. `array/symbol.iterator`) is correctly classified as a directory rather than
    ///   re-parsed as a file with extension `"iterator"`.
    /// - **Path with anchor** (`anc_sep.is_some()`): returns the filepath (anchor stripped)
    ///   via [`AnchorPath::new`] — the filepath may itself be a file.
    /// - **Directory** (`ext_sep.is_none()`, `anc_sep.is_none()`): walks up one level via
    ///   [`AnchorPath::new`] (already a directory, no dotted last component to misclassify).
    pub fn parent(&self) -> AnchorPath<'a> {
        if self.anc_sep.is_some() {
            AnchorPath::new(self.filepath())
        } else if self.ext_sep.is_some() {
            AnchorPath::new_dir(self.dir())
        } else {
            let dir_sep = self.dir_sep.unwrap_or(self.path.len());
            let next_sep = self.path[0..dir_sep].rfind('/');
            AnchorPath::new(&self.path[0..next_sep.unwrap_or(0)])
        }
    }

    /// Get the file extension from a URL path (anchor and parameter-aware)
    ///
    /// Strips any query parameters and anchor fragment before extracting the extension.
    ///
    /// See tests module for examples.
    pub fn ext(&self) -> &'a str {
        let stop_idx = self.param_sep.or(self.anc_sep).unwrap_or(self.path.len());
        let start_idx = self.ext_sep.map(|idx| idx + 1).unwrap_or(stop_idx);
        // Guard: new_file() sets ext_sep = path_end, making start_idx = path_end + 1 > stop_idx.
        // In that case there is no extension — return an empty slice.
        if start_idx > stop_idx {
            return &self.path[stop_idx..stop_idx];
        }
        &self.path[start_idx..stop_idx]
    }

    /// Get both filestem and extension as a tuple
    ///
    /// Returns (filestem, extension) where both are string slices.
    /// For files without extensions, ext will be an empty string.
    /// For paths treated as directories, filestem will be empty
    /// but the filename can be retrieved separately.
    ///
    /// # Examples
    /// ```
    /// use noet_core::paths::path::AnchorPath;
    ///
    /// let ap = AnchorPath::new("dir/file.md");
    /// assert_eq!(ap.path_parts(), ("file", "md"));
    ///
    /// // For extensionless files, filestem is empty (treated as directory)
    /// // Use filename() to get the actual name
    /// let ap = AnchorPath::new(".hidden");
    /// assert_eq!(ap.path_parts(), ("", ""));
    /// assert_eq!(ap.filename(), "");
    ///
    /// let ap = AnchorPath::new("dir/.noet#anchor");
    /// assert_eq!(ap.path_parts(), ("", ""));
    /// ```
    pub fn path_parts(&self) -> (&'a str, &'a str) {
        (self.filestem(), self.ext())
    }

    pub fn filepath(&self) -> &'a str {
        let start_idx = self
            .host_sep
            .or_else(|| self.sch_sep.map(|idx| idx + 1))
            .unwrap_or(0);
        if self.ext_sep.is_some() {
            let end_idx = self.param_sep.or(self.anc_sep).unwrap_or(self.path.len());
            &self.path[start_idx..end_idx]
        } else {
            self.dir()
        }
    }

    /// Calculate the join between two AnchorPaths.
    /// See tests module for examples.
    pub fn join<E: AsRef<str>>(&self, end_ref: E) -> AnchorPathBuf {
        let end = AnchorPath::from(end_ref.as_ref());
        if end.is_absolute() {
            return AnchorPathBuf::new(end.to_string());
        }
        if end.path.is_empty() {
            return AnchorPathBuf::new(self.to_string());
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
        AnchorPathBuf::new(res)
    }

    /// Normalize a URL path by resolving `.` and `..` components
    ///
    /// Preserves leading `..` components (standard path normalization behavior).
    /// Callers should check the result if they need to validate against backtracking.
    ///
    /// See tests module for examples.
    pub fn normalize(&self) -> AnchorPathBuf {
        let mut components = Vec::new();
        let mut final_components = Vec::new();
        let mut pop_dist = 0;
        for (idx, part) in self.filepath().split('/').enumerate() {
            match part {
                "" => {
                    if idx == 0 {
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
                        final_components.append(&mut vec![".."; pop_diff]);
                        components.clear();
                        pop_dist = 0;
                    } else if pop_dist > 0 {
                        let idx = components.len() - pop_dist;
                        let keep_part = part == components[idx];
                        if keep_part {
                            push_part = false;
                            pop_dist -= 1;
                        } else {
                            while pop_dist > 0 {
                                pop_dist -= 1;
                                let res = components.pop();
                                debug_assert!(res.is_some());
                            }
                        }
                    }
                    if push_part {
                        components.push(part);
                    }
                }
            }
        }
        if pop_dist > components.len() {
            let pop_diff = pop_dist - components.len();
            final_components.append(&mut vec![".."; pop_diff]);
            components.clear();
        } else {
            for _ in 0..pop_dist {
                components.pop();
            }
        }
        final_components.append(&mut components);

        let filepath = final_components.join("/");

        // Reconstruct the URL prefix (scheme + authority) if present.
        // filepath() strips these, so we restore them here to preserve
        // the full URL structure while normalizing only the path component.
        let prefix = if let Some(host_end) = self.host_sep {
            // Hierarchical URL (scheme://authority) — preserve everything up to the path
            &self.path[..host_end]
        } else if let Some(sch_idx) = self.sch_sep {
            // Non-hierarchical URL (scheme:) — preserve the scheme prefix
            &self.path[..sch_idx + 1]
        } else {
            ""
        };

        let params = self.parameters();
        let anchor = self.anchor();
        let mut res = format!("{prefix}{filepath}");
        if !params.is_empty() {
            res = format!("{res}?{params}");
        }
        if !anchor.is_empty() {
            res = format!("{res}#{anchor}");
        }
        AnchorPathBuf::new(res)
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
        let normalized_from: String = if rooted && self.is_absolute() {
            AnchorPath::new(self.path.trim_start_matches('/'))
                .normalize()
                .into()
        } else {
            self.normalize().into()
        };
        let normalized_to: String = if rooted {
            AnchorPath::new(to_ref.as_ref().trim_start_matches('/'))
                .normalize()
                .into()
        } else {
            AnchorPath::new(to_ref.as_ref()).normalize().into()
        };
        let from_clean = AnchorPath::from(&normalized_from);
        let to_clean = AnchorPath::from(&normalized_to);

        // Check if to_path starts with anchor - handle same-document anchors
        if to_clean.path.starts_with('#')
            || to_clean.is_absolute() && !from_clean.is_absolute()
            || (rooted && to_clean.path.starts_with("../"))
        {
            return normalized_to;
        }

        let joined_string: String = if !rooted {
            from_clean.join(to_clean).into()
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
        // Build relative path
        let mut result = Vec::new();

        // Add ../ for each remaining directory in from_path

        if from_parts.len() > common_len {
            for _ in common_len..from_parts.len() {
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

    /// Return a canonical root-relative path string: `"dir/file.ext#anchor"` with no leading slash
    /// and no schema/hostname prefix and no url params.
    ///
    /// This is the form that `get_nav_tree()` stores in `NavNode.path` and that
    /// `generate_terminal_path` produces for PathMap lookups. Use it whenever you neet to compare a
    /// parsed URL against a NavNode path without ad-hoc string manipulation.
    ///
    /// # Examples
    /// ```text
    /// "/net/doc.html#intro"            -> "net/doc.html#intro"
    /// "/index.html"                    -> "index.html"
    /// "doc.html#section"               -> "doc.html#section
    /// (external URLS have no meaningful root-relative form)
    /// "https://example.com/some/path"  -> ""
    /// ""                               -> ""
    /// ```
    pub fn canonicalize(&self) -> String {
        // External URLs (has schema) have no meaningful root-relative form.
        if self.has_schema() {
            return String::new();
        }
        // Strip leading '/' for Unix absolute paths, or "X:/" for Windows absolute paths.
        let filepath = self.filepath();
        let stripped = if filepath.len() >= 3
            && filepath.as_bytes()[0].is_ascii_alphabetic()
            && filepath.as_bytes()[1] == b':'
            && filepath.as_bytes()[2] == b'/'
        {
            &filepath[3..]
        } else {
            filepath.trim_start_matches('/')
        };
        format!("{}{}", stripped, as_anchor(self.anchor()))
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

impl<'a> PartialEq<str> for AnchorPath<'a> {
    fn eq(&self, other: &str) -> bool {
        self.path == other
    }
}

impl<'a> PartialEq<&str> for AnchorPath<'a> {
    fn eq(&self, other: &&str) -> bool {
        self.path == *other
    }
}

impl<'a> PartialEq<String> for AnchorPath<'a> {
    fn eq(&self, other: &String) -> bool {
        self.path == other.as_str()
    }
}

impl<'a> PartialEq<AnchorPath<'a>> for str {
    fn eq(&self, other: &AnchorPath<'a>) -> bool {
        self == other.path
    }
}

impl<'a> PartialEq<AnchorPath<'a>> for &str {
    fn eq(&self, other: &AnchorPath<'a>) -> bool {
        *self == other.path
    }
}

impl<'a> PartialEq<AnchorPath<'a>> for String {
    fn eq(&self, other: &AnchorPath<'a>) -> bool {
        self.as_str() == other.path
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

/// Owned version of [`AnchorPath`] — stores a `String` and pre-computes all separator indices.
///
/// This eliminates the re-parse overhead when chaining operations like `join` → accessor or
/// `normalize` → `join`. All accessor methods from `AnchorPath` are available via
/// [`as_anchor_path()`](AnchorPathBuf::as_anchor_path).
///
/// # Examples
/// ```
/// use noet_core::paths::path::AnchorPathBuf;
///
/// let buf = AnchorPathBuf::new("docs/guide.md".to_string());
/// assert_eq!(buf.as_anchor_path().dir(), "docs");
/// assert_eq!(buf.as_anchor_path().filename(), "guide.md");
///
/// // Deref to str for direct string operations
/// assert!(buf.starts_with("docs"));
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct AnchorPathBuf {
    path: String,
    /// Index of ':' separating schema from path
    sch_sep: Option<usize>,
    /// Index marking end of hostname (if present after schema://)
    host_sep: Option<usize>,
    /// Index of '?' separating path from query parameters
    param_sep: Option<usize>,
    /// Index of '/' separating path from file
    dir_sep: Option<usize>,
    /// Index of '.' separating filename from extension
    ext_sep: Option<usize>,
    /// Index of '#' separating path from anchor
    anc_sep: Option<usize>,
}

impl AnchorPathBuf {
    /// Create a new `AnchorPathBuf` by parsing the given owned string.
    pub fn new(path: String) -> AnchorPathBuf {
        let ap = AnchorPath::new(&path);
        AnchorPathBuf {
            sch_sep: ap.sch_sep,
            host_sep: ap.host_sep,
            param_sep: ap.param_sep,
            dir_sep: ap.dir_sep,
            ext_sep: ap.ext_sep,
            anc_sep: ap.anc_sep,
            path,
        }
    }

    /// Borrow as an [`AnchorPath`] for access to all parsed accessor methods.
    pub fn as_anchor_path(&self) -> AnchorPath<'_> {
        AnchorPath {
            path: &self.path,
            sch_sep: self.sch_sep,
            host_sep: self.host_sep,
            param_sep: self.param_sep,
            dir_sep: self.dir_sep,
            ext_sep: self.ext_sep,
            anc_sep: self.anc_sep,
        }
    }

    /// Consume self and return the underlying `String`.
    pub fn into_string(self) -> String {
        self.path
    }

    /// Join this path with another, returning an owned `AnchorPathBuf`.
    ///
    /// Delegates to [`AnchorPath::join`] — see that method for semantics.
    pub fn join<E: AsRef<str>>(&self, end_ref: E) -> AnchorPathBuf {
        self.as_anchor_path().join(end_ref)
    }

    /// Normalize this path, returning an owned `AnchorPathBuf`.
    ///
    /// Delegates to [`AnchorPath::normalize`] — see that method for semantics.
    pub fn normalize(&self) -> AnchorPathBuf {
        self.as_anchor_path().normalize()
    }

    /// Join in place — equivalent to `*self = self.join(end)`.
    ///
    /// Replaces this path with the result of joining `end` onto it,
    /// recomputing all cached indices. Mirrors `PathBuf::push` semantics.
    pub fn push<E: AsRef<str>>(&mut self, end: E) {
        *self = self.as_anchor_path().join(end);
    }
}

impl Deref for AnchorPathBuf {
    type Target = str;

    fn deref(&self) -> &str {
        &self.path
    }
}

impl AsRef<str> for AnchorPathBuf {
    fn as_ref(&self) -> &str {
        &self.path
    }
}

impl Display for AnchorPathBuf {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.path)
    }
}

impl From<String> for AnchorPathBuf {
    fn from(s: String) -> AnchorPathBuf {
        AnchorPathBuf::new(s)
    }
}

impl From<&str> for AnchorPathBuf {
    fn from(s: &str) -> AnchorPathBuf {
        AnchorPathBuf::new(s.to_string())
    }
}

impl From<AnchorPathBuf> for String {
    fn from(buf: AnchorPathBuf) -> String {
        buf.path
    }
}

impl PartialEq<str> for AnchorPathBuf {
    fn eq(&self, other: &str) -> bool {
        self.path == other
    }
}

impl PartialEq<&str> for AnchorPathBuf {
    fn eq(&self, other: &&str) -> bool {
        self.path == *other
    }
}

impl PartialEq<String> for AnchorPathBuf {
    fn eq(&self, other: &String) -> bool {
        self.path == *other
    }
}

impl PartialEq<AnchorPathBuf> for str {
    fn eq(&self, other: &AnchorPathBuf) -> bool {
        self == other.path
    }
}

impl PartialEq<AnchorPathBuf> for &str {
    fn eq(&self, other: &AnchorPathBuf) -> bool {
        *self == other.path
    }
}

impl PartialEq<AnchorPathBuf> for String {
    fn eq(&self, other: &AnchorPathBuf) -> bool {
        *self == other.path
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
        // parent() on a file in a dotted directory returns an AnchorPath<'_> with is_dir() = true
        let symbol_file = AnchorPath::new("/repo/array/symbol.iterator/index.md");
        let symbol_parent = symbol_file.parent();
        assert_eq!(symbol_parent, "/repo/array/symbol.iterator");
        assert!(symbol_parent.is_dir());
        assert_eq!(symbol_parent.ext(), "");
        assert_eq!(symbol_parent.path_parts(), ("", ""));
        assert_eq!(AnchorPath::new(".hidden_file.pdf").ext(), "pdf");
        assert_eq!(AnchorPath::new("noextension#anchor").ext(), "");
    }

    #[test]
    fn test_new_dir() {
        // new() misclassifies dotted directory names as files with extensions
        assert_eq!(AnchorPath::new("array/symbol.iterator").ext(), "iterator");
        assert_eq!(AnchorPath::new("array/symbol.iterator").is_dir(), false);

        // new_dir() corrects this: dotted last component is treated as a directory
        let ap = AnchorPath::new_dir("array/symbol.iterator");
        assert_eq!(ap.ext(), "");
        assert_eq!(ap.filestem(), "");
        assert_eq!(ap.filename(), "");
        assert_eq!(ap.is_dir(), true);
        assert_eq!(ap.dir(), "array/symbol.iterator");
        assert_eq!(ap.filepath(), "array/symbol.iterator");
        assert_eq!(ap.path_parts(), ("", ""));

        // Absolute path variant
        let ap = AnchorPath::new_dir("/repo/array/symbol.species");
        assert_eq!(ap.ext(), "");
        assert_eq!(ap.is_dir(), true);
        assert_eq!(ap.dir(), "/repo/array/symbol.species");
        assert_eq!(ap.filepath(), "/repo/array/symbol.species");

        // Paths that new() already treats as directories are unchanged
        let ap = AnchorPath::new_dir("plain_dir");
        assert_eq!(ap.ext(), "");
        assert_eq!(ap.is_dir(), true);

        let ap = AnchorPath::new_dir("nested/plain_dir");
        assert_eq!(ap.ext(), "");
        assert_eq!(ap.is_dir(), true);
        assert_eq!(ap.dir(), "nested/plain_dir");

        // Trailing slash: unchanged (already a directory)
        let ap = AnchorPath::new_dir("array/symbol.iterator/");
        assert_eq!(ap.ext(), "");
        assert_eq!(ap.is_dir(), true);

        // Real files are also correctly handled (though callers should use new() for files)
        let ap = AnchorPath::new_dir("dir/index.md");
        assert_eq!(ap.ext(), "");
        assert_eq!(ap.is_dir(), true);
        assert_eq!(ap.dir(), "dir/index.md");

        // Hidden dotted components (already treated as dir by new()) are unchanged
        let ap = AnchorPath::new_dir("network/.hidden_dir");
        assert_eq!(ap.ext(), "");
        assert_eq!(ap.is_dir(), true);

        // path_parts returns ("", "") — correct for codec (None, None) wildcard lookup
        assert_eq!(
            AnchorPath::new_dir("array/symbol.iterator").path_parts(),
            ("", "")
        );
        assert_eq!(
            AnchorPath::new_dir("array/symbol.asyncdispose").path_parts(),
            ("", "")
        );
    }

    #[test]
    fn test_join() {
        // Relative path joining
        let ap = AnchorPath::from("docs/guide.md");
        assert_eq!(ap.dir(), "docs");
        assert_eq!(ap.filestem(), "guide");
        assert_eq!(ap.filepath(), "docs/guide.md");
        assert_eq!(ap.parent(), "docs");
        assert!(ap.parent().is_dir());
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
        assert_eq!(AnchorPath::from("dir/.././file.md").normalize(), "file.md");
        assert_eq!(
            AnchorPath::from("/dir/.././file.md").normalize(),
            "/file.md"
        );
        assert_eq!(
            AnchorPath::from("..//../dir/.././file.md").normalize(),
            "../../file.md"
        );
        assert_eq!(
            AnchorPath::from("/dir/.//file.md").normalize(),
            "/dir/file.md"
        );

        // Anchor preservation
        assert_eq!(
            AnchorPath::from("dir/../file.md#section").normalize(),
            "file.md#section"
        );
        assert_eq!(
            AnchorPath::from("./file.md#anchor").normalize(),
            "file.md#anchor"
        );

        // Parameter preservation
        assert_eq!(
            AnchorPath::from("dir/../file.md?page=2").normalize(),
            "file.md?page=2"
        );
        assert_eq!(
            AnchorPath::from("./file.md?version=3").normalize(),
            "file.md?version=3"
        );

        // Both parameters and anchor preserved (correct order: ?params#anchor)
        assert_eq!(
            AnchorPath::from("dir/../file.md?page=2#section").normalize(),
            "file.md?page=2#section"
        );
        assert_eq!(
            AnchorPath::from("a/b/../c/file.md?x=1&y=2#frag").normalize(),
            "a/c/file.md?x=1&y=2#frag"
        );

        // Parameters without anchor
        assert_eq!(
            AnchorPath::from("/abs/../path/file.md?q=search").normalize(),
            "/path/file.md?q=search"
        );

        // Anchor without parameters (was already working)
        assert_eq!(
            AnchorPath::from("/abs/../path/file.md#heading").normalize(),
            "/path/file.md#heading"
        );

        // URL normalization — scheme + hostname preserved, path component normalized
        assert_eq!(
            AnchorPath::from("https://example.com/foo/../bar").normalize(),
            "https://example.com/bar"
        );
        assert_eq!(
            AnchorPath::from("https://example.com/a/b/../c/file.md").normalize(),
            "https://example.com/a/c/file.md"
        );
        assert_eq!(
            AnchorPath::from("https://example.com/path?q=test#section").normalize(),
            "https://example.com/path?q=test#section"
        );
        assert_eq!(
            AnchorPath::from("https://example.com/a/../b?x=1#frag").normalize(),
            "https://example.com/b?x=1#frag"
        );
        // URL with no path component — preserved as-is
        assert_eq!(
            AnchorPath::from("https://example.com").normalize(),
            "https://example.com"
        );
        // Non-hierarchical URL — scheme prefix preserved
        assert_eq!(
            AnchorPath::from("mailto:user@example.com").normalize(),
            "mailto:user@example.com"
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

        let rel =
            AnchorPath::from("docs/guide.md").path_to("../..//../docs/../reference/./api.md", true);
        assert_eq!(rel, "../../../reference/api.md");
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
    fn test_canonicalize() {
        assert_eq!(
            AnchorPath::from("/net/doc.html#intro").canonicalize(),
            "net/doc.html#intro"
        );
        assert_eq!(AnchorPath::from("/index.html").canonicalize(), "index.html");
        assert_eq!(
            AnchorPath::from("net/doc.html").canonicalize(),
            "net/doc.html"
        );
        assert_eq!(
            AnchorPath::from("doc.html#section").canonicalize(),
            "doc.html#section"
        );
        assert_eq!(AnchorPath::from("#section").canonicalize(), "#section");
        assert_eq!(AnchorPath::from("doc.html").canonicalize(), "doc.html");
        assert_eq!(AnchorPath::from("").canonicalize(), "");
        assert_eq!(
            AnchorPath::from("https://example.com/page#anchor").canonicalize(),
            ""
        );
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
        // Basic lowercasing and whitespace handling
        assert_eq!(to_anchor("Details"), "details");
        assert_eq!(to_anchor("Section One"), "section-one");
        assert_eq!(to_anchor("My-Section!"), "my-section");

        // & is stripped; surrounding spaces each become a hyphen which then collapse
        assert_eq!(to_anchor("API & Reference"), "api-reference");

        // Semantically meaningful JS API punctuation is preserved
        assert_eq!(to_anchor("Temporal.Duration"), "temporal.duration");
        assert_eq!(to_anchor("Temporal.Duration()"), "temporal.duration()");
        assert_eq!(to_anchor("Array.prototype.map()"), "array.prototype.map()");
        assert_eq!(to_anchor("Symbol.iterator"), "symbol.iterator");
        assert_eq!(
            to_anchor("Object.__defineGetter__()"),
            "object.__definegetter__()"
        );
        assert_eq!(to_anchor("for...in"), "for...in");
        assert_eq!(to_anchor("try...catch"), "try...catch");
        assert_eq!(to_anchor("Array[@@iterator]()"), "array[@@iterator]()");

        // Critical: constructor page no longer collides with its parent class
        assert_ne!(
            to_anchor("Temporal.Duration"),
            to_anchor("Temporal.Duration()")
        );
        assert_ne!(
            to_anchor("Array.prototype.map"),
            to_anchor("Array.prototype.map()")
        );

        // Case insensitivity
        assert_eq!(to_anchor("Details"), to_anchor("DETAILS"));
        assert_eq!(to_anchor("Section"), to_anchor("section"));

        // Hyphen collapsing
        assert_eq!(to_anchor("a  b"), "a-b");
        assert_eq!(to_anchor("--leading"), "leading");
        assert_eq!(to_anchor("trailing--"), "trailing");

        // NFKC: compatibility variants resolved without stripping accents
        assert_eq!(to_anchor("ﬁle"), "file"); // ligature fi → fi
        assert_eq!(to_anchor("²nd"), "2nd"); // superscript 2 → 2
        assert_eq!(to_anchor("ℕatural"), "natural"); // double-struck N → N

        // Non-ASCII alphanumeric preserved (accents, CJK, etc.)
        assert_eq!(to_anchor("Résumé"), "résumé");
        assert_eq!(to_anchor("für"), "für");
        assert_eq!(to_anchor("日本語"), "日本語");

        // Accented and unaccented titles stay distinct — no silent collision
        assert_ne!(to_anchor("Résumé"), to_anchor("Resume"));
        assert_ne!(to_anchor("für"), to_anchor("fur"));
        assert_ne!(to_anchor("café"), to_anchor("cafe"));

        // NFC/NFD precomposed vs decomposed → same anchor via NFKC
        // é as precomposed U+00E9 vs e + combining acute U+0301
        let precomposed = "\u{00E9}"; // é (single codepoint)
        let decomposed = "e\u{0301}"; // e + combining acute
        assert_eq!(to_anchor(precomposed), to_anchor(decomposed));
    }

    #[test]
    fn test_schema_parsing() {
        // Basic schema parsing
        let ap = AnchorPath::new("http://example.com/path/file.md");
        assert!(ap.has_schema());
        assert!(ap.has_hostname());
        assert_eq!(ap.schema(), "http");
        assert_eq!(ap.hostname(), "example.com");
        assert_eq!(ap.dir(), "/path");
        assert_eq!(ap.filename(), "file.md");
        assert_eq!(ap.filepath(), "/path/file.md");

        let ap = AnchorPath::new("https://example.com/dir/");
        assert!(ap.has_schema());
        assert!(ap.has_hostname());
        assert_eq!(ap.schema(), "https");
        assert_eq!(ap.hostname(), "example.com");
        assert_eq!(ap.dir(), "/dir");
        assert_eq!(ap.filepath(), "/dir");

        let ap = AnchorPath::new("file:///absolute/path/file.txt");
        assert!(ap.has_schema());
        assert!(ap.has_hostname());
        assert_eq!(ap.schema(), "file");
        assert_eq!(ap.hostname(), "");
        assert_eq!(ap.dir(), "/absolute/path");
        assert_eq!(ap.filename(), "file.txt");
        assert_eq!(ap.filepath(), "/absolute/path/file.txt");

        // Schema with anchor
        let ap = AnchorPath::new("https://example.com/doc.html#section");
        assert!(ap.has_schema());
        assert!(ap.has_hostname());
        assert_eq!(ap.schema(), "https");
        assert_eq!(ap.hostname(), "example.com");
        assert_eq!(ap.dir(), "");
        assert_eq!(ap.filename(), "doc.html");
        assert_eq!(ap.anchor(), "section");
        assert_eq!(ap.filepath(), "/doc.html");

        // No schema (colon after slash)
        let ap = AnchorPath::new("/path/with:colon/file.md");
        assert!(!ap.has_schema());
        assert!(!ap.has_hostname());
        assert_eq!(ap.schema(), "");
        assert_eq!(ap.hostname(), "");
        assert_eq!(ap.dir(), "/path/with:colon");
        assert_eq!(ap.filename(), "file.md");

        // No schema (plain path)
        let ap = AnchorPath::new("dir/file.md");
        assert!(!ap.has_schema());
        assert!(!ap.has_hostname());
        assert_eq!(ap.schema(), "");
        assert_eq!(ap.hostname(), "");
        assert_eq!(ap.dir(), "dir");
        assert_eq!(ap.filename(), "file.md");
    }

    #[test]
    fn test_parameter_parsing() {
        // Basic parameter parsing
        let ap = AnchorPath::new("dir/file.md?page=2");
        assert!(ap.has_parameters());
        assert_eq!(ap.parameters(), "page=2");
        assert_eq!(ap.dir(), "dir");
        assert_eq!(ap.filename(), "file.md");
        assert_eq!(ap.filepath(), "dir/file.md");
        assert_eq!(ap.ext(), "md");

        // Parameters with anchor
        let ap = AnchorPath::new("dir/file.html?id=123#section");
        assert!(ap.has_parameters());
        assert_eq!(ap.parameters(), "id=123");
        assert_eq!(ap.anchor(), "section");
        assert_eq!(ap.dir(), "dir");
        assert_eq!(ap.filename(), "file.html");
        assert_eq!(ap.filepath(), "dir/file.html");

        // Multiple parameters
        let ap = AnchorPath::new("api/endpoint.json?page=2&sort=desc&limit=10");
        assert!(ap.has_parameters());
        assert_eq!(ap.parameters(), "page=2&sort=desc&limit=10");
        assert_eq!(ap.filename(), "endpoint.json");
        assert_eq!(ap.ext(), "json");

        // Parameters without file extension
        let ap = AnchorPath::new("api/endpoint?query=test");
        assert!(ap.has_parameters());
        assert_eq!(ap.parameters(), "query=test");
        assert_eq!(ap.dir(), "api/endpoint");
        assert_eq!(ap.filename(), "");

        // No parameters
        let ap = AnchorPath::new("dir/file.md");
        assert!(!ap.has_parameters());
        assert_eq!(ap.parameters(), "");
    }

    #[test]
    fn test_schema_and_parameters() {
        // Full URL with schema, parameters, and anchor
        let ap = AnchorPath::new("https://example.com/api/data.json?page=2&sort=asc#results");
        assert!(ap.has_schema());
        assert!(ap.has_hostname());
        assert!(ap.has_parameters());
        assert_eq!(ap.schema(), "https");
        assert_eq!(ap.hostname(), "example.com");
        assert_eq!(ap.parameters(), "page=2&sort=asc");
        assert_eq!(ap.anchor(), "results");
        assert_eq!(ap.dir(), "/api");
        assert_eq!(ap.filename(), "data.json");
        assert_eq!(ap.filepath(), "/api/data.json");
        assert_eq!(ap.ext(), "json");

        // Schema and parameters, no anchor
        let ap = AnchorPath::new("http://localhost:8080/index.html?debug=true");
        assert!(ap.has_schema());
        assert!(ap.has_hostname());
        assert!(ap.has_parameters());
        assert_eq!(ap.schema(), "http");
        assert_eq!(ap.hostname(), "localhost:8080");
        assert_eq!(ap.parameters(), "debug=true");
        assert_eq!(ap.anchor(), "");
        assert_eq!(ap.filepath(), "/index.html");

        // Schema with anchor, no parameters
        let ap = AnchorPath::new("ftp://server.com/files/doc.pdf#page-5");
        assert!(ap.has_schema());
        assert!(ap.has_hostname());
        assert!(!ap.has_parameters());
        assert_eq!(ap.schema(), "ftp");
        assert_eq!(ap.hostname(), "server.com");
        assert_eq!(ap.parameters(), "");
        assert_eq!(ap.anchor(), "page-5");
        assert_eq!(ap.filepath(), "/files/doc.pdf");

        // Parameters without schema
        let ap = AnchorPath::new("/absolute/path?param=value");
        assert!(!ap.has_schema());
        assert!(!ap.has_hostname());
        assert!(ap.has_parameters());
        assert_eq!(ap.schema(), "");
        assert_eq!(ap.hostname(), "");
        assert_eq!(ap.parameters(), "param=value");
        assert_eq!(ap.dir(), "/absolute/path");

        // Edge case: question mark in anchor
        let ap = AnchorPath::new("file.md#what?");
        assert!(!ap.has_parameters());
        assert_eq!(ap.anchor(), "what?");
        assert_eq!(ap.parameters(), "");
    }

    #[test]
    fn test_schema_edge_cases() {
        // Colon in query parameter should not be detected as schema
        let ap = AnchorPath::new("path/file.md?time=12:30");
        assert!(!ap.has_schema());
        assert_eq!(ap.parameters(), "time=12:30");

        // Colon in anchor should not be detected as schema
        let ap = AnchorPath::new("file.md#time:12:30");
        assert!(!ap.has_schema());
        assert_eq!(ap.anchor(), "time:12:30");

        // Custom schema
        let ap = AnchorPath::new("custom-protocol://path/file");
        assert!(ap.has_schema());
        assert_eq!(ap.schema(), "custom-protocol");

        // Single letter followed by colon is a Windows drive letter — NOT a URL schema.
        // AnchorPath now treats it as a plain absolute path, preserving the drive prefix.
        let ap = AnchorPath::new("c:/Windows/file.txt");
        assert!(
            !ap.has_schema(),
            "drive letter must not be detected as schema"
        );
        assert_eq!(ap.schema(), "");
        assert_eq!(ap.filepath(), "c:/Windows/file.txt");
        assert_eq!(ap.dir(), "c:/Windows");
        assert!(ap.is_absolute());

        // Empty schema with //
        let ap = AnchorPath::new("://path");
        assert!(ap.has_schema());
        assert!(ap.has_hostname());
        assert_eq!(ap.schema(), "");
        assert_eq!(ap.hostname(), "path");
        assert_eq!(ap.filepath(), "");
        assert_eq!(ap.dir(), "");
    }

    #[test]
    fn test_complex_url_components() {
        // Complex real-world URL
        let ap = AnchorPath::new(
            "https://docs.rs/url/2.5.0/url/struct.Url.html?search=parse#method.join",
        );
        assert!(ap.has_schema());
        assert!(ap.has_hostname());
        assert!(ap.has_parameters());
        assert_eq!(ap.schema(), "https");
        assert_eq!(ap.hostname(), "docs.rs");
        assert_eq!(ap.dir(), "/url/2.5.0/url");
        assert_eq!(ap.filename(), "struct.Url.html");
        assert_eq!(ap.filestem(), "struct.Url");
        assert_eq!(ap.ext(), "html");
        assert_eq!(ap.parameters(), "search=parse");
        assert_eq!(ap.anchor(), "method.join");
        assert_eq!(ap.filepath(), "/url/2.5.0/url/struct.Url.html");

        // Data URL (cannot-be-a-base in standard URL parsing)
        let ap = AnchorPath::new("data:text/plain,HelloWorld");
        assert!(ap.has_schema());
        assert!(!ap.has_hostname());
        assert_eq!(ap.schema(), "data");
        assert_eq!(ap.hostname(), "");
        // After schema, the rest is treated as path
        assert_eq!(ap.filepath(), "text/plain,HelloWorld");
        assert_eq!(ap.dir(), "text/plain,HelloWorld");

        // Mailto URL
        let ap = AnchorPath::new("mailto:user@example.com?subject=Hello");
        assert!(ap.has_schema());
        assert!(!ap.has_hostname());
        assert!(ap.has_parameters());
        assert_eq!(ap.schema(), "mailto");
        assert_eq!(ap.hostname(), "");
        assert_eq!(ap.filepath(), "user@example.com");
        assert_eq!(ap.parameters(), "subject=Hello");
        assert_eq!(ap.dir(), "");
    }

    #[test]
    fn test_url_without_path() {
        // URL with no path component (just scheme and host)
        let ap = AnchorPath::new("https://google.com");
        assert!(ap.has_schema());
        assert!(ap.has_hostname());
        assert_eq!(ap.schema(), "https");
        assert_eq!(ap.hostname(), "google.com");
        assert_eq!(ap.dir(), "");
        assert_eq!(ap.filepath(), "");
        assert_eq!(ap.filename(), "");
        assert_eq!(ap.filestem(), "");
        assert_eq!(ap.ext(), "");

        // URL with trailing slash - has empty path
        let ap = AnchorPath::new("https://google.com/");
        assert!(ap.has_schema());
        assert!(ap.has_hostname());
        assert_eq!(ap.schema(), "https");
        assert_eq!(ap.hostname(), "google.com");
        assert_eq!(ap.dir(), "/");
        assert_eq!(ap.filepath(), "/");
        assert_eq!(ap.filename(), "");
    }

    #[test]
    fn test_windows_absolute_paths() {
        // AnchorPath now treats single-char alpha + ':' as a Windows drive letter,
        // NOT a URL schema.  filepath() and dir() include the full "C:/" prefix,
        // is_absolute() returns true, has_schema() returns false.

        // Basic file path
        let ap = AnchorPath::new("C:/Users/RUNNER/AppData/Local/Temp/test.md");
        assert!(
            !ap.has_schema(),
            "drive letter must not be detected as schema"
        );
        assert!(!ap.has_hostname(), "C:/ must not produce a hostname");
        assert_eq!(ap.schema(), "");
        assert_eq!(ap.hostname(), "");
        assert_eq!(ap.dir(), "C:/Users/RUNNER/AppData/Local/Temp");
        assert_eq!(ap.filename(), "test.md");
        assert_eq!(ap.filestem(), "test");
        assert_eq!(ap.ext(), "md");
        assert_eq!(ap.filepath(), "C:/Users/RUNNER/AppData/Local/Temp/test.md");
        assert!(ap.is_absolute());

        // Directory path (no extension)
        let ap = AnchorPath::new("C:/Users/RUNNER/AppData/Local/Temp/subnet1");
        assert!(!ap.has_schema());
        assert!(!ap.has_hostname());
        assert_eq!(ap.dir(), "C:/Users/RUNNER/AppData/Local/Temp/subnet1");
        assert_eq!(ap.filename(), "");
        assert_eq!(ap.filestem(), "");
        assert_eq!(ap.filepath(), "C:/Users/RUNNER/AppData/Local/Temp/subnet1");
        assert!(ap.is_absolute());

        // Network index path
        let ap = AnchorPath::new("C:/Users/RUNNER/AppData/Local/Temp/subnet1/index.md");
        assert!(!ap.has_schema());
        assert!(!ap.has_hostname());
        assert_eq!(ap.dir(), "C:/Users/RUNNER/AppData/Local/Temp/subnet1");
        assert_eq!(ap.filename(), "index.md");
        assert_eq!(ap.filestem(), "index");
        assert_eq!(ap.ext(), "md");
        assert_eq!(
            ap.filepath(),
            "C:/Users/RUNNER/AppData/Local/Temp/subnet1/index.md"
        );

        // With anchor fragment
        let ap = AnchorPath::new("C:/Users/RUNNER/AppData/Local/Temp/test.md#section");
        assert!(!ap.has_schema());
        assert!(!ap.has_hostname());
        assert_eq!(ap.filepath(), "C:/Users/RUNNER/AppData/Local/Temp/test.md");
        assert_eq!(ap.anchor(), "section");
        assert!(ap.is_absolute());

        // Lowercase drive letter
        let ap = AnchorPath::new("c:/tmp/project/docs/guide.md");
        assert!(!ap.has_schema());
        assert!(!ap.has_hostname());
        assert_eq!(ap.dir(), "c:/tmp/project/docs");
        assert_eq!(ap.filestem(), "guide");
        assert_eq!(ap.ext(), "md");
        assert!(ap.is_absolute());

        // normalize() must preserve the drive-letter prefix
        let ap = AnchorPath::new("C:/Users/foo/../bar/./baz.md");
        assert_eq!(
            ap.normalize().as_anchor_path().filepath(),
            "C:/Users/bar/baz.md"
        );

        // join() with a relative path must preserve the drive-letter base
        let base = AnchorPath::new("C:/Users/foo/doc.md");
        let joined = base.join("../assets/img.png");
        assert_eq!(
            joined.as_anchor_path().filepath(),
            "C:/Users/assets/img.png"
        );

        // starts_with check (mirrors regularize_unchecked guard)
        let base_abs = "C:/Users/RUNNER/repo";
        let link_abs = "C:/Users/RUNNER/repo/assets/img.png";
        assert!(
            AnchorPath::new(link_abs).path.starts_with(base_abs),
            "drive-letter path must start_with its base"
        );

        // Real URL schemas (>=2 chars) must still parse hostname correctly
        let ap = AnchorPath::new("https://example.com/path/file.html");
        assert!(ap.has_schema());
        assert!(ap.has_hostname());
        assert_eq!(ap.schema(), "https");
        assert_eq!(ap.hostname(), "example.com");
        assert_eq!(ap.filepath(), "/path/file.html");

        let ap = AnchorPath::new("file:///tmp/test.md");
        assert!(ap.has_schema());
        assert_eq!(ap.schema(), "file");
        assert_eq!(ap.filepath(), "/tmp/test.md");
    }

    #[test]
    fn test_non_hierarchical_urls() {
        // Non-hierarchical URLs don't have // after schema, so no hostname parsing

        // Mailto - everything after : is the path
        let ap = AnchorPath::new("mailto:sandy@acme.corp");
        assert!(ap.has_schema());
        assert!(!ap.has_hostname());
        assert_eq!(ap.schema(), "mailto");
        assert_eq!(ap.hostname(), "");
        assert_eq!(ap.filepath(), "sandy@acme.corp");
        assert_eq!(ap.dir(), "");

        // Data URL
        let ap = AnchorPath::new("data:text/html,<h1>Hello</h1>");
        assert!(ap.has_schema());
        assert!(!ap.has_hostname());
        assert_eq!(ap.schema(), "data");
        assert_eq!(ap.hostname(), "");
        assert_eq!(ap.filepath(), "text/html,<h1>Hello</h1>");

        // Javascript (rare but valid)
        let ap = AnchorPath::new("javascript:alert('test')");
        assert!(ap.has_schema());
        assert!(!ap.has_hostname());
        assert_eq!(ap.schema(), "javascript");
        assert_eq!(ap.hostname(), "");
        assert_eq!(ap.filepath(), "alert('test')");

        // Tel (telephone)
        let ap = AnchorPath::new("tel:+1-555-1234");
        assert!(ap.has_schema());
        assert!(!ap.has_hostname());
        assert_eq!(ap.schema(), "tel");
        assert_eq!(ap.hostname(), "");
        assert_eq!(ap.filepath(), "+1-555-1234");
    }
}
