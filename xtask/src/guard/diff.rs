//! Parsing what git reports into what the rules read, and the path globs
//! the protected list and trailers use.

/// One changed path, from `git diff --name-status -M -z`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// `A`, `C`, `D`, `M`, `R` or `T`, the similarity score dropped.
    pub status: char,
    /// The path at base, absent for an added or copied file.
    pub old: Option<String>,
    /// The path at head, absent for a deleted file.
    pub new: Option<String>,
}

impl Change {
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        let new = self
            .new
            .as_deref()
            .filter(|new| self.old.as_deref() != Some(*new));
        self.old.as_deref().into_iter().chain(new)
    }
}

/// The removed and added lines of one file, from `git diff -U0`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileLines {
    pub old: Option<String>,
    pub new: Option<String>,
    /// Base line numbers (1-based) and text.
    pub removed: Vec<(usize, String)>,
    /// Head line numbers (1-based) and text.
    pub added: Vec<(usize, String)>,
}

/// The entries of a path list: one per line, `#` comments and blanks
/// skipped.
pub fn patterns(text: &str) -> Vec<String> {
    text.lines()
        .map(|line| line.split('#').next().unwrap_or_default().trim())
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

pub fn name_status(text: &str) -> Vec<Change> {
    let mut fields = text.split('\0').filter(|field| !field.is_empty());
    let mut changes = Vec::new();
    while let Some(status) = fields.next() {
        let Some(code) = status.chars().next() else {
            continue;
        };
        let first = fields.next().map(str::to_owned);
        let change = match code {
            'R' | 'C' => {
                let second = fields.next().map(str::to_owned);
                let old = if code == 'R' { first } else { None };
                Change {
                    status: code,
                    old,
                    new: second,
                }
            }
            'A' => Change {
                status: code,
                old: None,
                new: first,
            },
            'D' => Change {
                status: code,
                old: first,
                new: None,
            },
            _ => Change {
                status: code,
                old: first.clone(),
                new: first,
            },
        };
        changes.push(change);
    }
    changes
}

/// Parses a zero-context unified diff into per-file removed and added
/// lines. `---` and `+++` name the paths only in a file's header, before
/// its first hunk, so a removed `-- x` line is a line.
pub fn unified(text: &str) -> Vec<FileLines> {
    let mut files: Vec<FileLines> = Vec::new();
    let (mut old_line, mut new_line, mut header) = (0, 0, false);
    for line in text.lines() {
        if line.starts_with("diff --git ") {
            files.push(FileLines::default());
            header = true;
        } else if let Some(hunk) = line.strip_prefix("@@ ") {
            (old_line, new_line) = hunk_starts(hunk);
            header = false;
        } else if header {
            if let Some(path) = line.strip_prefix("--- ") {
                set(&mut files, path, |file| &mut file.old);
            } else if let Some(path) = line.strip_prefix("+++ ") {
                set(&mut files, path, |file| &mut file.new);
            }
        } else if let (Some(text), Some(file)) = (line.strip_prefix('-'), files.last_mut()) {
            file.removed.push((old_line, text.to_owned()));
            old_line += 1;
        } else if let (Some(text), Some(file)) = (line.strip_prefix('+'), files.last_mut()) {
            file.added.push((new_line, text.to_owned()));
            new_line += 1;
        }
    }
    files
}

fn set(files: &mut [FileLines], path: &str, field: fn(&mut FileLines) -> &mut Option<String>) {
    let Some(file) = files.last_mut() else {
        return;
    };
    let path = path.trim_end_matches('\t');
    let path = path
        .strip_prefix('"')
        .and_then(|path| path.strip_suffix('"'))
        .unwrap_or(path);
    let path = path.strip_prefix("a/").or_else(|| path.strip_prefix("b/"));
    *field(file) = path.map(str::to_owned);
}

/// The first line numbers of a hunk header's two ranges
/// (`-12,3 +14,0 @@`). An empty range's number is the line before it,
/// and no line of it is read.
fn hunk_starts(hunk: &str) -> (usize, usize) {
    let mut ranges = hunk.split(' ').take(2).map(|range| {
        range
            .get(1..)
            .unwrap_or_default()
            .split(',')
            .next()
            .and_then(|start| start.parse().ok())
            .unwrap_or(0)
    });
    (ranges.next().unwrap_or(0), ranges.next().unwrap_or(0))
}

/// Whether `path` matches a glob: `*` matches within a segment and `**`
/// matches any number of whole segments.
pub fn matches(pattern: &str, path: &str) -> bool {
    let pattern: Vec<_> = pattern.split('/').collect();
    let path: Vec<_> = path.split('/').collect();
    matches_segments(&pattern, &path)
}

fn matches_segments(pattern: &[&str], path: &[&str]) -> bool {
    match (pattern.split_first(), path.split_first()) {
        (None, None) => true,
        (Some((&"**", rest)), _) => {
            (0..=path.len()).any(|skip| matches_segments(rest, &path[skip..]))
        }
        (Some((segment, rest)), Some((name, names))) => {
            matches_segment(segment.as_bytes(), name.as_bytes()) && matches_segments(rest, names)
        }
        _ => false,
    }
}

fn matches_segment(pattern: &[u8], name: &[u8]) -> bool {
    match pattern.split_first() {
        None => name.is_empty(),
        Some((b'*', rest)) => (0..=name.len()).any(|skip| matches_segment(rest, &name[skip..])),
        Some((byte, rest)) => name
            .split_first()
            .is_some_and(|(first, names)| first == byte && matches_segment(rest, names)),
    }
}

/// A test file: `tests.rs`, `testing.rs`, or any file under a `tests`
/// directory.
pub fn is_test_file(path: &str) -> bool {
    let mut segments: Vec<_> = path.split('/').collect();
    let name = segments.pop().unwrap_or_default();
    matches!(name, "tests.rs" | "testing.rs") || segments.contains(&"tests")
}

/// The directories holding a `Cargo.toml`, `""` for the root.
pub fn crate_directories<'a>(files: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut directories: Vec<_> = files
        .filter_map(|file| match file.rsplit_once('/') {
            Some((directory, "Cargo.toml")) => Some(directory.to_owned()),
            None if file == "Cargo.toml" => Some(String::new()),
            _ => None,
        })
        .collect();
    directories.sort();
    directories
}

/// The deepest crate directory holding `path`, `.` for the root.
pub fn crate_of(crates: &[String], path: &str) -> String {
    let owner = crates
        .iter()
        .filter(|directory| !directory.is_empty() && path.starts_with(&format!("{directory}/")))
        .max_by_key(|directory| directory.len());
    owner.map_or_else(|| ".".to_owned(), String::clone)
}

/// One commit of the range: its message, its `Retires:` and
/// `Asserts-changed:` trailer values (exact paths), and the paths it
/// changes (a merge, against its first parent).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Commit {
    pub message: String,
    pub retires: Vec<String>,
    pub asserts_changed: Vec<String>,
    pub paths: Vec<String>,
}

/// The format `log` takes so [`commits`] can read it: per commit,
/// `\x01<message>\x02<trailers>\x02` then the NUL-separated paths.
pub const LOG_FORMAT: &str = "--format=%x01%B%x02%(trailers:only,unfold)%x02";

pub fn commits(text: &str) -> Vec<Commit> {
    text.split('\u{1}')
        .filter(|record| !record.is_empty())
        .map(|record| {
            let mut parts = record.splitn(3, '\u{2}');
            let message = parts.next().unwrap_or_default().to_owned();
            let trailers = parts.next().unwrap_or_default();
            let paths = parts
                .next()
                .unwrap_or_default()
                .split('\0')
                .map(|path| path.trim_matches('\n'))
                .filter(|path| !path.is_empty())
                .map(str::to_owned)
                .collect();
            let values = |key: &str| -> Vec<String> {
                trailers
                    .lines()
                    .filter_map(|line| line.strip_prefix(key))
                    .flat_map(|values| values.split([',', ' ', '\t']))
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
                    .collect()
            };
            Commit {
                message,
                retires: values("Retires:"),
                asserts_changed: values("Asserts-changed:"),
                paths,
            }
        })
        .collect()
}

/// The blobs of a `cat-file --batch` reply to `specs`, by spec; a missing
/// object, or one that is not a blob, is left out.
pub fn batch(
    specs: &[String],
    mut reply: &[u8],
) -> Result<std::collections::BTreeMap<String, String>, crate::TaskError> {
    let error = || crate::TaskError::Message("git cat-file --batch: reply not understood".into());
    let mut blobs = std::collections::BTreeMap::new();
    for spec in specs {
        let end = reply
            .iter()
            .position(|&byte| byte == b'\n')
            .ok_or_else(error)?;
        let header = String::from_utf8_lossy(&reply[..end]).into_owned();
        reply = &reply[end + 1..];
        if header.ends_with(" missing") || header.ends_with(" ambiguous") {
            continue;
        }
        let mut fields = header.split(' ').skip(1);
        let kind = fields.next().ok_or_else(error)?;
        let size: usize = fields
            .next()
            .and_then(|size| size.parse().ok())
            .ok_or_else(error)?;
        let body = reply.get(..size).ok_or_else(error)?;
        if kind == "blob" {
            blobs.insert(spec.clone(), String::from_utf8_lossy(body).into_owned());
        }
        reply = reply.get(size + 1..).ok_or_else(error)?;
    }
    Ok(blobs)
}
