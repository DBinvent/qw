//! §10 demo: run the cold-start bootstrap analyzer against a real git
//! repository and print what it suggests. Nothing here is signed or
//! published — see `qw_node::bootstrap`'s module docs for why.
//!
//! Usage: `cargo run -p qw-node --example bootstrap_from_git -- [repo_path]`
//! (defaults to `.`)

use std::path::PathBuf;

use qw_node::bootstrap::analyze_repo;

fn main() {
    let repo_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));

    let suggestions = match analyze_repo(&repo_path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Could not analyze {}: {e}", repo_path.display());
            std::process::exit(1);
        }
    };

    println!(
        "Bootstrap suggestions for {} ({} contributor(s) seen):",
        repo_path.display(),
        suggestions.len()
    );
    println!("(candidates only — nothing here is signed or published; each contributor reviews their own before signing)\n");

    for s in &suggestions {
        println!(
            "- {} <{}> — {} commit(s)",
            s.contributor.name, s.contributor.email, s.commit_count
        );
        if !s.candidate_skill_tags.is_empty() {
            println!(
                "    candidate skill tags: {}",
                s.candidate_skill_tags.join(", ")
            );
        }
        if !s.candidate_introductions.is_empty() {
            let names: Vec<String> = s
                .candidate_introductions
                .iter()
                .map(|c| c.name.clone())
                .collect();
            println!("    candidate introductions: {}", names.join(", "));
        }
    }
}
