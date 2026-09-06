//! The skill taxonomy and its synonyms, bundled into the client.
//!
//! NIP-QW03 requires free-text skill input to be normalized through
//! `/synonyms.yaml` *before* a profile event is signed — tag
//! fragmentation ("nodejs" vs "node.js") cannot be undone once it is in a
//! signed record. So the profile editor is a picker over `/taxonomy.yaml`
//! leaves, with synonyms resolving anything the user types.
//!
//! **Payload decision:** both files are `include_str!`d into the binary
//! rather than fetched. They are ~18 KB together and change rarely (adding
//! a *domain* is a protocol change; skills are an open set resolved here).
//! The cost is that they only refresh with an app release — acceptable, and
//! the same property everything else in this client already has.

use std::collections::BTreeMap;
use std::sync::OnceLock;

use serde::Deserialize;

const TAXONOMY_YAML: &str = include_str!("../../../taxonomy.yaml");
const SYNONYMS_YAML: &str = include_str!("../../../synonyms.yaml");

// --- raw file shapes -----------------------------------------------------

#[derive(Deserialize)]
struct TaxonomyFile {
    sectors: BTreeMap<String, Sector>,
}
#[derive(Deserialize)]
struct Sector {
    #[serde(default)]
    domains: BTreeMap<String, Domain>,
}
#[derive(Deserialize)]
struct Domain {
    /// A narrow domain lists skills directly; a wider one groups them into
    /// areas. `/taxonomy.yaml`: "area — Optional".
    #[serde(default)]
    skills: Vec<String>,
    #[serde(default)]
    areas: BTreeMap<String, Area>,
}
#[derive(Deserialize)]
struct Area {
    #[serde(default)]
    skills: Vec<String>,
}
#[derive(Deserialize)]
struct SynonymsFile {
    synonyms: BTreeMap<String, Vec<String>>,
}

// --- parsed, indexed ---------------------------------------------------

struct Model {
    /// Every leaf tag, `sector/domain[/area]#skill`, sorted and unique.
    leaves: Vec<String>,
    /// Bare skill name -> the leaf tag(s) carrying it. Usually one; a skill
    /// that appears under two areas has several and cannot be placed from
    /// free text alone.
    by_skill: BTreeMap<String, Vec<String>>,
    /// alias -> canonical skill name (from synonyms.yaml).
    alias: BTreeMap<String, String>,
}

fn model() -> &'static Model {
    static M: OnceLock<Model> = OnceLock::new();
    M.get_or_init(|| {
        let tax: TaxonomyFile =
            serde_yaml::from_str(TAXONOMY_YAML).expect("bundled taxonomy.yaml parses");
        let syn: SynonymsFile =
            serde_yaml::from_str(SYNONYMS_YAML).expect("bundled synonyms.yaml parses");

        let mut leaves = Vec::new();
        let mut by_skill: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut push = |path: String, skill: &str| {
            let tag = format!("{path}#{skill}");
            leaves.push(tag.clone());
            by_skill.entry(skill.to_string()).or_default().push(tag);
        };
        for (sector, s) in &tax.sectors {
            for (domain, d) in &s.domains {
                for skill in &d.skills {
                    push(format!("{sector}/{domain}"), skill);
                }
                for (area, a) in &d.areas {
                    for skill in &a.skills {
                        push(format!("{sector}/{domain}/{area}"), skill);
                    }
                }
            }
        }
        leaves.sort();
        leaves.dedup();

        let mut alias = BTreeMap::new();
        for (canonical, aliases) in &syn.synonyms {
            for a in aliases {
                alias.insert(a.to_lowercase(), canonical.clone());
            }
        }

        Model { leaves, by_skill, alias }
    })
}

/// Every leaf tag in the taxonomy, sorted — the source list for the
/// editor's picker.
pub fn leaves() -> &'static [String] {
    &model().leaves
}

/// What a piece of user input resolved to.
#[derive(Debug, Clone, PartialEq)]
pub enum Resolved {
    /// A leaf that exists in `/taxonomy.yaml` — routable.
    Canonical(String),
    /// A well-formed `sector/domain[/area]#skill` tag whose skill is not in
    /// the taxonomy yet. Legal (the skill set is open) but unroutable until
    /// the taxonomy catches up. NIP-QW03 §"unknown values are legal".
    Unrouted(String),
}

impl Resolved {
    pub fn tag(&self) -> &str {
        match self {
            Resolved::Canonical(t) | Resolved::Unrouted(t) => t,
        }
    }
}

/// The synonyms.yaml normalization steps for free text: lowercase, trim,
/// whitespace to `-`, drop an `@version` qualifier.
fn normalize_free_text(input: &str) -> String {
    let base = input.split('@').next().unwrap_or(input);
    base.trim()
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
}

/// Resolve one editor entry — a full tag from the picker, or something the
/// user typed — to a canonical or explicitly-unrouted leaf. `Err` when it
/// cannot be placed (unknown, or an ambiguous bare skill name).
pub fn resolve(input: &str) -> Result<Resolved, String> {
    let m = model();
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err("empty".into());
    }

    // A full `…/…#…` tag: from the picker, or typed deliberately.
    if trimmed.contains('#') && trimmed.contains('/') {
        let full = normalize_free_text(trimmed);
        return Ok(if m.leaves.iter().any(|l| l == &full) {
            Resolved::Canonical(full)
        } else {
            Resolved::Unrouted(full)
        });
    }

    // Free text: a bare skill name or an alias for one.
    let word = normalize_free_text(trimmed);
    let skill = m.alias.get(&word).cloned().unwrap_or(word);
    match m.by_skill.get(&skill).map(Vec::as_slice) {
        Some([only]) => Ok(Resolved::Canonical(only.clone())),
        Some(many) if !many.is_empty() => Err(format!(
            "`{skill}` is under more than one area — pick one: {}",
            many.join(", ")
        )),
        _ => Err(format!(
            "`{input}` is not a known skill — pick from the list, or type the full \
             sector/domain#skill form to publish it as an unrouted tag"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bundled_files_parse_and_yield_leaves() {
        let l = leaves();
        assert!(l.len() > 50, "only {} leaves", l.len());
        assert!(l.contains(&"it/backend/languages#rust".to_string()));
        // sorted + unique
        assert!(l.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn a_picker_tag_resolves_to_itself() {
        assert_eq!(
            resolve("it/backend/languages#rust"),
            Ok(Resolved::Canonical("it/backend/languages#rust".into()))
        );
    }

    #[test]
    fn free_text_resolves_through_synonyms() {
        assert_eq!(
            resolve("Rust Lang"),
            Ok(Resolved::Canonical("it/backend/languages#rust".into()))
        );
        assert_eq!(
            resolve("golang"),
            Ok(Resolved::Canonical("it/backend/languages#go".into()))
        );
        // version qualifier stripped
        assert_eq!(
            resolve("rust@1.80"),
            Ok(Resolved::Canonical("it/backend/languages#rust".into()))
        );
    }

    #[test]
    fn an_unknown_full_tag_is_unrouted_not_rejected() {
        match resolve("it/backend/languages#zig") {
            Ok(Resolved::Unrouted(t)) => assert_eq!(t, "it/backend/languages#zig"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn unknown_free_text_is_rejected() {
        assert!(resolve("underwater basket weaving").is_err());
    }
}
