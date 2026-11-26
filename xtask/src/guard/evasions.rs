//! The evasion corpus: each test commits one way to weaken the oracles to a
//! scratch repository and checks the guard names it, and the last checks
//! that the honest forms of the same changes pass.

use super::{Git, rules};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

pub const LIB: &str = "pub fn one() -> u8 {\n    1\n}\n\n#[cfg(test)]\nmod tests;\n";
pub const TESTS: &str = "\
#[test]
fn adds() {
    assert_eq!(
        1 + 1,
        2,
    );
}

#[test]
fn subtracts() {
    assert!(2 - 1 == 1);
}

#[test]
fn subtracts_again() {
    assert!(2 - 1 == 1);
}
";

pub struct Repo {
    pub root: PathBuf,
    pub base: String,
}

impl Repo {
    pub fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!("xtask-guard-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let mut repo = Self {
            root,
            base: String::new(),
        };
        repo.write(
            "xtask/guard/protected.txt",
            "tests/oracle/**\n**/.cargo/**\n**/.gitattributes\n",
        );
        repo.write("Cargo.toml", "[package]\nname = \"a\"\n");
        repo.write("src/lib.rs", LIB);
        repo.write("src/tests.rs", TESTS);
        repo.git(&["init", "-q"]);
        repo.commit("base");
        repo.base = repo.git(&["rev-parse", "HEAD"]);
        repo
    }

    /// Commits the scratch files as a new base for the change under test.
    pub fn rebase(&mut self, files: &[(&str, &str)]) {
        for (path, text) in files {
            self.write(path, text);
        }
        self.commit("base");
        self.base = self.git(&["rev-parse", "HEAD"]);
    }

    pub fn git(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args([
                "-c",
                "user.name=guard",
                "-c",
                "user.email=guard@example.invalid",
            ])
            .args([
                "-c",
                "commit.gpgsign=false",
                "-c",
                "core.hooksPath=/dev/null",
            ])
            .args(args)
            .output();
        let output = output.ok().filter(|output| output.status.success());
        assert!(output.is_some(), "git {args:?}");
        output
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
            .unwrap_or_default()
    }

    pub fn write(&self, path: &str, text: &str) {
        let path = self.root.join(path);
        let made = path
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| fs::write(&path, text));
        assert!(made.is_ok(), "{}", path.display());
    }

    pub fn edit(&self, path: &str, from: &str, to: &str) {
        let text = fs::read_to_string(self.root.join(path)).unwrap_or_default();
        assert!(text.contains(from), "{path}: {from}");
        self.write(path, &text.replacen(from, to, 1));
    }

    pub fn commit(&self, message: &str) {
        self.git(&["add", "-A"]);
        self.git(&["commit", "-q", "--allow-empty", "-m", message]);
    }

    /// Each violation as `<rule> <path>`.
    pub fn violations(&self) -> Vec<String> {
        let inputs = Git { root: &self.root }.inputs(&self.base, "HEAD");
        assert!(inputs.is_ok(), "{inputs:?}");
        let mut found: Vec<_> = inputs
            .map(|inputs| rules::check(&inputs))
            .unwrap_or_default()
            .iter()
            .map(|line| {
                let mut words = line.split_whitespace().skip(1);
                let rule = words.next().unwrap_or_default().trim_end_matches(':');
                let path = words.next().unwrap_or_default().trim_end_matches(':');
                let path = path.split(':').next().unwrap_or_default();
                format!("{rule} {path}")
            })
            .collect();
        found.sort();
        found
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[cfg(test)]
mod tests;
