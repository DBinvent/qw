//! Cold-start bootstrap from real git history (§10, `todo-impl.md`
//! "Bootstrap trust graph from real historical collaboration ... rather
//! than asking a cold network to self-report from zero").
//!
//! This analyzes a real git repository's commit history and produces
//! **suggestions** — candidate skill tags (from which file extensions a
//! contributor touched most) and candidate `Introduction` pairs (from
//! `Co-authored-by:` trailers, real evidence two people worked directly
//! together). It does *not* fabricate or publish anything on anyone's
//! behalf: git identifies people by name/email, not by a QW `did:key`,
//! and this tool holds no one's signing key. The intended flow is a
//! human onboarding step — each real contributor, once they generate
//! their own `Identity`, reviews their own suggested skill tags and
//! introductions and signs (`qw_protocol::events::kinds::
//! profile_skill_tags` / `introduction`) whatever they actually confirm,
//! themselves.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::path::Path;
use std::process::Command;

const RECORD_SEP: char = '\u{2}';
const UNIT_SEP: char = '\u{1f}';
const BODY_END: char = '\u{3}';

/// Identity is by **email only**, case-insensitively — real repos have
/// the same person committing under inconsistent name capitalization or
/// formatting far more often than they have two people sharing an email
/// (confirmed against this project's own history: "Vladimir Krinitsyn"
/// and "vkrinitsyn" are the same contributor). `name` is a display field,
/// not part of identity; when the same email appears under multiple
/// names, whichever name was encountered first in `git log`'s (newest
/// first) output order is kept.
#[derive(Debug, Clone)]
pub struct Contributor {
    pub name: String,
    pub email: String,
}

impl PartialEq for Contributor {
    fn eq(&self, other: &Self) -> bool {
        self.email.eq_ignore_ascii_case(&other.email)
    }
}

impl Eq for Contributor {}

impl std::hash::Hash for Contributor {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.email.to_ascii_lowercase().hash(state);
    }
}

#[derive(Debug, Clone, Default)]
struct ContributorStats {
    commit_count: u32,
    extensions_touched: HashMap<String, u32>,
    co_authored_with: HashSet<Contributor>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BootstrapSuggestion {
    pub contributor: Contributor,
    pub commit_count: u32,
    /// Ranked by how many touched files matched that extension —
    /// candidates for the contributor to review/edit before signing
    /// their own `ProfileSkillTags`, never auto-published.
    pub candidate_skill_tags: Vec<String>,
    /// Other contributors seen on a shared `Co-authored-by:` trailer —
    /// candidates for an `Introduction`, same caveat.
    pub candidate_introductions: Vec<Contributor>,
}

#[derive(Debug)]
pub enum BootstrapError {
    GitCommandFailed(std::io::Error),
    NotAGitRepository,
}

impl fmt::Display for BootstrapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BootstrapError::GitCommandFailed(e) => write!(f, "could not run git: {e}"),
            BootstrapError::NotAGitRepository => {
                write!(f, "not a git repository (or git log failed)")
            }
        }
    }
}

impl std::error::Error for BootstrapError {}

/// Analyze `repo_path`'s commit history and return one suggestion per
/// contributor seen, sorted by commit count descending.
pub fn analyze_repo(repo_path: &Path) -> Result<Vec<BootstrapSuggestion>, BootstrapError> {
    let format = format!("{RECORD_SEP}%H{UNIT_SEP}%an{UNIT_SEP}%ae{UNIT_SEP}%B{BODY_END}");
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .arg("log")
        .arg(format!("--pretty=format:{format}"))
        .arg("--name-only")
        .output()
        .map_err(BootstrapError::GitCommandFailed)?;

    if !output.status.success() {
        return Err(BootstrapError::NotAGitRepository);
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut stats: HashMap<Contributor, ContributorStats> = HashMap::new();

    for block in text.split(RECORD_SEP).filter(|b| !b.trim().is_empty()) {
        let Some((header_and_body, files_part)) = block.split_once(BODY_END) else {
            continue;
        };
        let mut fields = header_and_body.splitn(4, UNIT_SEP);
        let (Some(_hash), Some(name), Some(email), Some(body)) =
            (fields.next(), fields.next(), fields.next(), fields.next())
        else {
            continue;
        };

        let contributor = Contributor {
            name: name.trim().to_string(),
            email: email.trim().to_string(),
        };
        let entry = stats.entry(contributor).or_default();
        entry.commit_count += 1;

        for line in files_part.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Some(ext) = Path::new(line).extension().and_then(|e| e.to_str()) {
                *entry
                    .extensions_touched
                    .entry(ext.to_lowercase())
                    .or_insert(0) += 1;
            }
        }

        for line in body.lines() {
            if let Some(rest) = line.trim().strip_prefix("Co-authored-by:") {
                if let Some(co) = parse_name_email(rest) {
                    entry.co_authored_with.insert(co);
                }
            }
        }
    }

    // Symmetrize co-authorship: a trailer on A's commit crediting B
    // should surface as a candidate introduction for B too, and give B
    // a (possibly commit_count == 0) entry even if git log never showed
    // them as the primary author.
    let reverse_edges: Vec<(Contributor, Contributor)> = stats
        .iter()
        .flat_map(|(author, s)| {
            s.co_authored_with
                .iter()
                .map(move |co| (co.clone(), author.clone()))
        })
        .collect();
    for (target, add) in reverse_edges {
        stats
            .entry(target)
            .or_default()
            .co_authored_with
            .insert(add);
    }

    let mut suggestions: Vec<BootstrapSuggestion> = stats
        .into_iter()
        .map(|(contributor, s)| {
            let mut ranked_extensions: Vec<(String, u32)> =
                s.extensions_touched.into_iter().collect();
            ranked_extensions.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
            let candidate_skill_tags: Vec<String> = ranked_extensions
                .iter()
                .filter_map(|(ext, _)| extension_to_skill_tag(ext))
                .map(str::to_string)
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();

            BootstrapSuggestion {
                contributor,
                commit_count: s.commit_count,
                candidate_skill_tags,
                candidate_introductions: s.co_authored_with.into_iter().collect(),
            }
        })
        .collect();

    suggestions.sort_by(|a, b| {
        b.commit_count
            .cmp(&a.commit_count)
            .then_with(|| a.contributor.name.cmp(&b.contributor.name))
    });
    Ok(suggestions)
}

fn parse_name_email(s: &str) -> Option<Contributor> {
    let s = s.trim();
    let (name, email) = s.rsplit_once('<')?;
    let email = email.strip_suffix('>')?;
    let name = name.trim();
    let email = email.trim();
    if name.is_empty() || email.is_empty() {
        return None;
    }
    Some(Contributor {
        name: name.to_string(),
        email: email.to_string(),
    })
}

/// Best-effort file-extension to `/taxonomy.yaml` leaf tag mapping — not
/// exhaustive, easy to extend. A contributor reviews and edits the
/// resulting suggestion before signing anything, so an imprecise or
/// missing mapping only means a weaker suggestion, never a wrong signed
/// record.
fn extension_to_skill_tag(ext: &str) -> Option<&'static str> {
    match ext {
        "rs" => Some("it/backend/languages#rust"),
        "go" => Some("it/backend/languages#go"),
        "py" => Some("it/backend/languages#python"),
        "java" => Some("it/backend/languages#java"),
        "kt" | "kts" => Some("it/backend/languages#kotlin"),
        "cs" => Some("it/backend/languages#csharp"),
        "rb" => Some("it/backend/languages#ruby"),
        "php" => Some("it/backend/languages#php"),
        "ex" | "exs" => Some("it/backend/languages#elixir"),
        "scala" => Some("it/backend/languages#scala"),
        "cpp" | "cc" | "cxx" | "hpp" => Some("it/backend/languages#cpp"),
        "c" | "h" => Some("it/embedded/firmware#c-embedded"),
        "js" | "jsx" | "mjs" => Some("it/frontend/fundamentals#javascript"),
        "ts" | "tsx" => Some("it/frontend/fundamentals#typescript"),
        "swift" => Some("it/mobile/native#swift"),
        "sql" => Some("it/data/analytics#sql"),
        "tf" => Some("it/infra/automation#terraform"),
        "sol" => Some("it/distributed/blockchain#solidity"),
        "md" => Some("it/docs#technical-writing"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as ProcessCommand;

    fn run(dir: &Path, args: &[&str]) {
        let status = ProcessCommand::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .status()
            .expect("git available");
        assert!(status.success(), "git {args:?} failed");
    }

    fn init_repo(dir: &Path) {
        run(dir, &["init", "-q"]);
        run(dir, &["config", "user.email", "alice@example.com"]);
        run(dir, &["config", "user.name", "Alice"]);
    }

    #[test]
    fn analyzes_commit_count_and_file_extensions() {
        let tmp = std::env::temp_dir().join(format!("qw-bootstrap-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        init_repo(&tmp);
        std::fs::write(tmp.join("main.rs"), "fn main() {}").unwrap();
        run(&tmp, &["add", "."]);
        run(&tmp, &["commit", "-q", "-m", "add main"]);
        std::fs::write(tmp.join("lib.py"), "pass").unwrap();
        run(&tmp, &["add", "."]);
        run(&tmp, &["commit", "-q", "-m", "add lib"]);

        let suggestions = analyze_repo(&tmp).unwrap();
        assert_eq!(suggestions.len(), 1);
        let alice = &suggestions[0];
        assert_eq!(alice.contributor.email, "alice@example.com");
        assert_eq!(alice.commit_count, 2);
        assert!(alice
            .candidate_skill_tags
            .contains(&"it/backend/languages#rust".to_string()));
        assert!(alice
            .candidate_skill_tags
            .contains(&"it/backend/languages#python".to_string()));

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn co_authored_by_trailer_becomes_a_symmetric_introduction_candidate() {
        let tmp =
            std::env::temp_dir().join(format!("qw-bootstrap-test-coauthor-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        init_repo(&tmp);
        std::fs::write(tmp.join("readme.md"), "hello").unwrap();
        run(&tmp, &["add", "."]);
        run(
            &tmp,
            &[
                "commit",
                "-q",
                "-m",
                "docs\n\nCo-authored-by: Bob <bob@example.com>",
            ],
        );

        let suggestions = analyze_repo(&tmp).unwrap();
        let alice = suggestions
            .iter()
            .find(|s| s.contributor.name == "Alice")
            .unwrap();
        let bob = suggestions
            .iter()
            .find(|s| s.contributor.email == "bob@example.com")
            .unwrap();

        assert!(alice
            .candidate_introductions
            .iter()
            .any(|c| c.email == "bob@example.com"));
        assert!(
            bob.candidate_introductions
                .iter()
                .any(|c| c.email == "alice@example.com"),
            "co-authorship must surface for both sides, not just the primary committer"
        );
        assert_eq!(
            bob.commit_count, 0,
            "bob never appears as a primary committer here, only as a co-author"
        );

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn non_git_directory_errors_cleanly() {
        let tmp =
            std::env::temp_dir().join(format!("qw-bootstrap-not-a-repo-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        assert!(analyze_repo(&tmp).is_err());
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn same_email_different_name_spelling_merges_into_one_contributor() {
        // Real-world case found by running this tool against an actual
        // multi-commit repo: the same person committed as both
        // "Vladimir Krinitsyn" and "vkrinitsyn" under one email.
        let tmp =
            std::env::temp_dir().join(format!("qw-bootstrap-name-variants-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        init_repo(&tmp);
        std::fs::write(tmp.join("a.rs"), "fn a() {}").unwrap();
        run(&tmp, &["add", "."]);
        run(&tmp, &["commit", "-q", "-m", "first"]);

        run(&tmp, &["config", "user.name", "alice"]);
        std::fs::write(tmp.join("b.rs"), "fn b() {}").unwrap();
        run(&tmp, &["add", "."]);
        run(&tmp, &["commit", "-q", "-m", "second"]);

        let suggestions = analyze_repo(&tmp).unwrap();
        assert_eq!(
            suggestions.len(),
            1,
            "same email under two name spellings must be one contributor"
        );
        assert_eq!(suggestions[0].commit_count, 2);

        std::fs::remove_dir_all(&tmp).ok();
    }
}
