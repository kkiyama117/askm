//! Fuzzy search across registered marketplaces' plugins and skills.
//!
//! Built on `nucleo-matcher`'s [`Pattern`] API. A [`Matcher`] carries ~135KB of
//! reusable scratch memory and is expensive to construct, so [`search`] builds
//! exactly one (plus one query [`Pattern`]) per call and threads both through
//! every candidate string, rather than rebuilding either inside the per-entry
//! or per-skill loop.

use std::cmp::Reverse;

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config as MatcherConfig, Matcher, Utf32Str};

use crate::model::{Marketplace, MarketplaceEntry};
use crate::state::{PluginId, State};

/// One ranked search hit: a plugin, plus the names of its skills (if any) that
/// also matched the query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResult {
    pub id: PluginId,
    pub version: Option<String>,
    pub description: Option<String>,
    /// Skill names within this plugin that matched, best match first.
    pub matching_skills: Vec<String>,
    pub score: u32,
}

/// Search every entry across `marketplaces` — pairs of (registered name,
/// already-synced [`Marketplace`]) — scoring against plugin name, description,
/// and keywords, plus each skill name known for the entry. `state` supplies
/// exact skill names for an installed plugin; for one not yet installed, the
/// final path component of each `explicit_skills` entry is used as a
/// best-effort skill name. Results are sorted by score descending and capped
/// at `limit`.
pub fn search(
    marketplaces: &[(String, Marketplace)],
    state: &State,
    query: &str,
    limit: usize,
) -> Vec<SearchResult> {
    let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
    let mut matcher = Matcher::new(MatcherConfig::DEFAULT);
    let mut buf = Vec::new();

    let mut results = Vec::new();
    for (marketplace_name, marketplace) in marketplaces {
        for entry in &marketplace.entries {
            let scored = score_entry(
                &pattern,
                &mut matcher,
                &mut buf,
                marketplace_name,
                entry,
                state,
            );
            if let Some(result) = scored {
                results.push(result);
            }
        }
    }

    results.sort_by_key(|r| {
        (
            Reverse(r.score),
            r.id.plugin.clone(),
            r.id.marketplace.clone(),
        )
    });
    results.truncate(limit);
    results
}

fn score_entry(
    pattern: &Pattern,
    matcher: &mut Matcher,
    buf: &mut Vec<char>,
    marketplace_name: &str,
    entry: &MarketplaceEntry,
    state: &State,
) -> Option<SearchResult> {
    let id = PluginId::new(entry.name.clone(), marketplace_name);
    let mut best = score_plugin_fields(pattern, matcher, buf, entry);

    let mut skill_hits: Vec<(String, u32)> = candidate_skill_names(state, &id, entry)
        .into_iter()
        .filter_map(|name| {
            let score = score_text(pattern, matcher, buf, &name)?;
            Some((name, score))
        })
        .collect();
    for (_, score) in &skill_hits {
        best = combine(best, Some(*score));
    }

    let score = best?;
    skill_hits.sort_by_key(|(_, score)| Reverse(*score));
    let matching_skills = skill_hits.into_iter().map(|(name, _)| name).collect();

    Some(SearchResult {
        id,
        version: entry.version.clone(),
        description: entry.description.clone(),
        matching_skills,
        score,
    })
}

fn score_plugin_fields(
    pattern: &Pattern,
    matcher: &mut Matcher,
    buf: &mut Vec<char>,
    entry: &MarketplaceEntry,
) -> Option<u32> {
    let mut best = score_text(pattern, matcher, buf, &entry.name);
    best = combine(
        best,
        score_text(
            pattern,
            matcher,
            buf,
            entry.description.as_deref().unwrap_or(""),
        ),
    );
    if !entry.keywords.is_empty() {
        let joined = entry.keywords.join(" ");
        best = combine(best, score_text(pattern, matcher, buf, &joined));
    }
    best
}

fn score_text(
    pattern: &Pattern,
    matcher: &mut Matcher,
    buf: &mut Vec<char>,
    text: &str,
) -> Option<u32> {
    if text.trim().is_empty() {
        return None;
    }
    pattern.score(Utf32Str::new(text, buf), matcher)
}

fn combine(current: Option<u32>, candidate: Option<u32>) -> Option<u32> {
    match (current, candidate) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(a), None) => Some(a),
        (None, b) => b,
    }
}

/// Skill names to score against: exact recorded names for an installed
/// plugin, else a best-effort guess from the marketplace entry's declared
/// `explicit_skills` paths (e.g. `./skills/box` -> `box`).
fn candidate_skill_names(state: &State, id: &PluginId, entry: &MarketplaceEntry) -> Vec<String> {
    if let Some(installed) = state.installed(id) {
        return installed.skills.clone();
    }
    entry
        .explicit_skills
        .iter()
        .filter_map(|raw| {
            raw.rsplit('/')
                .next()
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Dialect, EntrySource};
    use crate::state::InstalledPlugin;

    fn entry(name: &str, description: &str, keywords: &[&str]) -> MarketplaceEntry {
        MarketplaceEntry {
            name: name.to_string(),
            version: Some("1.0.0".to_string()),
            description: Some(description.to_string()),
            source: EntrySource::Local {
                path: "./".to_string(),
            },
            category: None,
            keywords: keywords.iter().map(|s| s.to_string()).collect(),
            homepage: None,
            author: None,
            explicit_skills: Vec::new(),
        }
    }

    fn marketplace(name: &str, entries: Vec<MarketplaceEntry>) -> Marketplace {
        Marketplace {
            name: name.to_string(),
            display_name: None,
            description: None,
            entries,
            dialect: Dialect::Agents,
        }
    }

    #[test]
    fn search_ranks_a_name_match_above_an_unrelated_entry() {
        let marketplaces = vec![(
            "official".to_string(),
            marketplace(
                "official",
                vec![
                    entry(
                        "systematic-debugging",
                        "Debug things carefully.",
                        &["debug"],
                    ),
                    entry("brainstorming", "Explore ideas.", &["ideation"]),
                ],
            ),
        )];

        let results = search(&marketplaces, &State::default(), "debug", 10);

        assert!(!results.is_empty());
        assert_eq!(results[0].id.plugin, "systematic-debugging");
    }

    #[test]
    fn search_matches_on_description_and_keywords_too() {
        let marketplaces = vec![(
            "official".to_string(),
            marketplace(
                "official",
                vec![entry("box", "A skill about origami.", &["paper-folding"])],
            ),
        )];

        let by_description = search(&marketplaces, &State::default(), "origami", 10);
        let by_keyword = search(&marketplaces, &State::default(), "paper-folding", 10);

        assert_eq!(by_description.len(), 1);
        assert_eq!(by_keyword.len(), 1);
    }

    #[test]
    fn search_reports_matching_skill_names_from_installed_state() {
        let marketplaces = vec![(
            "official".to_string(),
            marketplace("official", vec![entry("superpowers", "Core skills.", &[])]),
        )];
        let state = State::default().record_install(InstalledPlugin {
            plugin: "superpowers".to_string(),
            marketplace: "official".to_string(),
            version: "6.2.0".to_string(),
            source: EntrySource::Local {
                path: "./".to_string(),
            },
            installed_at: 0,
            skills: vec![
                "systematic-debugging".to_string(),
                "brainstorming".to_string(),
            ],
        });

        let results = search(&marketplaces, &state, "debugging", 10);

        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].matching_skills,
            vec!["systematic-debugging".to_string()]
        );
    }

    #[test]
    fn search_excludes_entries_that_do_not_match_at_all() {
        let marketplaces = vec![(
            "official".to_string(),
            marketplace("official", vec![entry("box", "Origami skill.", &[])]),
        )];

        let results = search(&marketplaces, &State::default(), "zzz-nonexistent-zzz", 10);

        assert!(results.is_empty());
    }

    #[test]
    fn search_respects_the_limit() {
        let entries: Vec<_> = (0..5)
            .map(|i| entry(&format!("demo-{i}"), "Demo plugin.", &[]))
            .collect();
        let marketplaces = vec![("official".to_string(), marketplace("official", entries))];

        let results = search(&marketplaces, &State::default(), "demo", 2);

        assert_eq!(results.len(), 2);
    }
}
