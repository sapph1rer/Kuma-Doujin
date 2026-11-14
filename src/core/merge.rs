//! Combining metadata from several sources with per-field provenance.
//!
//! Rules: the primary record (the source you read from) wins. A secondary source only fills
//! fields the primary left empty, and adds new tags and alternative titles. Nothing already
//! present is overwritten, so user edits applied on top of the primary are never replaced.

use crate::core::models::Manga;

fn mark(m: &mut Manga, field: &str, source: &str) {
    let list = m.provenance.entry(field.to_string()).or_default();
    if !list.iter().any(|s| s == source) {
        list.push(source.to_string());
    }
}

/// Records the primary source for every field it filled. Runs once: after a merge,
/// fields filled by secondaries must not be credited to the primary.
pub fn record_primary(m: &mut Manga) {
    if !m.provenance.is_empty() {
        return;
    }
    let src = m.key.source.clone();
    let filled = [
        ("title.english", m.titles.english.is_some()),
        ("title.japanese", m.titles.japanese.is_some()),
        ("title.thai", m.titles.thai.is_some()),
        ("title.alternatives", !m.titles.alternatives.is_empty()),
        ("description", m.description.is_some()),
        ("artists", !m.artists.is_empty()),
        ("circles", !m.circles.is_empty()),
        ("parodies", !m.parodies.is_empty()),
        ("characters", !m.characters.is_empty()),
        ("tags", !m.tags.is_empty()),
        ("language", m.language.is_some()),
        ("published", m.published.is_some()),
        ("page_count", m.page_count.is_some()),
        ("cover", m.cover_url.is_some()),
    ];
    for (field, present) in filled {
        if present {
            mark(m, field, &src);
        }
    }
}

fn fill<T: Clone>(slot: &mut Option<T>, from: &Option<T>) -> bool {
    if slot.is_none() && from.is_some() {
        *slot = from.clone();
        return true;
    }
    false
}

fn fill_list(slot: &mut Vec<String>, from: &[String]) -> bool {
    if slot.is_empty() && !from.is_empty() {
        *slot = from.to_vec();
        return true;
    }
    false
}

/// Appends values not already present (case-insensitive). Returns true if any were added.
fn union(slot: &mut Vec<String>, from: &[String]) -> bool {
    let mut added = false;
    for v in from {
        if !slot.iter().any(|x| x.eq_ignore_ascii_case(v)) {
            slot.push(v.clone());
            added = true;
        }
    }
    added
}

/// Merges `extra` (a secondary source) into `base` without overwriting anything.
pub fn merge(base: &mut Manga, extra: &Manga) {
    record_primary(base);
    let src = extra.key.source.as_str();
    let t = &extra.titles;
    let mut changed: Vec<&str> = Vec::new();
    if fill(&mut base.titles.english, &t.english) {
        changed.push("title.english");
    }
    if fill(&mut base.titles.japanese, &t.japanese) {
        changed.push("title.japanese");
    }
    if fill(&mut base.titles.thai, &t.thai) {
        changed.push("title.thai");
    }
    let known: Vec<String> = [
        &base.titles.english,
        &base.titles.japanese,
        &base.titles.thai,
    ]
    .into_iter()
    .flatten()
    .cloned()
    .collect();
    let new_alts: Vec<String> = t
        .alternatives
        .iter()
        .chain([&t.english, &t.japanese, &t.thai].into_iter().flatten())
        .filter(|a| !known.contains(a))
        .cloned()
        .collect();
    if union(&mut base.titles.alternatives, &new_alts) {
        changed.push("title.alternatives");
    }
    if fill(&mut base.description, &extra.description) {
        changed.push("description");
    }
    if fill_list(&mut base.artists, &extra.artists) {
        changed.push("artists");
    }
    if fill_list(&mut base.circles, &extra.circles) {
        changed.push("circles");
    }
    if fill_list(&mut base.parodies, &extra.parodies) {
        changed.push("parodies");
    }
    if fill_list(&mut base.characters, &extra.characters) {
        changed.push("characters");
    }
    if union(&mut base.tags, &extra.tags) {
        changed.push("tags");
    }
    if fill(&mut base.published, &extra.published) {
        changed.push("published");
    }
    if fill(&mut base.page_count, &extra.page_count) {
        changed.push("page_count");
    }
    if fill(&mut base.cover_url, &extra.cover_url) {
        changed.push("cover");
    }
    for field in changed {
        mark(base, field, src);
    }
    // The primary often already links the work (e.g. MangaDex `links.al`); still record the
    // secondary's URL so the details page can attribute it.
    if !base.links.iter().any(|(s, _)| s == src) {
        base.links.push((src.to_string(), extra.key.id.clone()));
    }
    if let Some(url) = &extra.source_url {
        mark(base, "link", src);
        mark(base, &format!("url.{src}"), url);
    }
}

/// Plain text from source descriptions: drops HTML tags, Markdown emphasis and links
/// (keeping link text), and MangaDex's trailing "---" links section.
pub fn clean_description(raw: &str) -> String {
    let mut s = raw.split("\n---").next().unwrap_or(raw).to_string();
    for (from, to) in [("<br>", "\n"), ("<br/>", "\n"), ("<br />", "\n")] {
        s = s.replace(from, to);
    }
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    // [text](url) -> text
    let mut res = String::with_capacity(out.len());
    let mut rest = out.as_str();
    while let Some(start) = rest.find('[') {
        let after = &rest[start + 1..];
        match (after.find("]("), after.find(']')) {
            (Some(mid), Some(close)) if mid == close => {
                let tail = &after[mid + 2..];
                match tail.find(')') {
                    Some(end) => {
                        res.push_str(&rest[..start]);
                        res.push_str(&after[..mid]);
                        rest = &tail[end + 1..];
                    }
                    None => break,
                }
            }
            _ => {
                res.push_str(&rest[..=start]);
                rest = after;
            }
        }
    }
    res.push_str(rest);
    let res = res.replace("**", "").replace("__", "");
    let lines: Vec<&str> = res.lines().map(str::trim_end).collect();
    let mut text = lines.join("\n");
    while text.contains("\n\n\n") {
        text = text.replace("\n\n\n", "\n\n");
    }
    text.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::models::{MangaKey, Titles};

    fn manga(source: &str) -> Manga {
        Manga {
            key: MangaKey {
                source: source.into(),
                id: "1".into(),
            },
            titles: Titles::default(),
            description: None,
            artists: vec![],
            circles: vec![],
            parodies: vec![],
            characters: vec![],
            tags: vec![],
            language: None,
            published: None,
            page_count: None,
            cover_url: None,
            source_url: None,
            links: vec![],
            provenance: Default::default(),
        }
    }

    #[test]
    fn merge_fills_gaps_never_overwrites_and_tracks_sources() {
        let mut base = manga("mangadex");
        base.links = vec![("anilist".into(), "1".into())]; // primary already links the work
        base.titles.english = Some("Solo Leveling".into());
        base.tags = vec!["Action".into()];
        base.artists = vec!["Chugong".into()];

        let mut extra = manga("anilist");
        extra.titles.english = Some("Other English".into());
        extra.titles.japanese = Some("俺だけレベルアップな件".into());
        extra.titles.alternatives = vec!["Only I Level Up".into()];
        extra.tags = vec!["action".into(), "Dungeon".into()];
        extra.artists = vec!["Someone Else".into()];
        extra.description = Some("From AniList".into());
        extra.source_url = Some("https://anilist.co/manga/1".into());

        merge(&mut base, &extra);
        assert_eq!(base.titles.english.as_deref(), Some("Solo Leveling"));
        assert_eq!(
            base.titles.japanese.as_deref(),
            Some("俺だけレベルアップな件")
        );
        assert!(
            base.titles
                .alternatives
                .contains(&"Other English".to_string())
        );
        assert_eq!(base.tags, ["Action", "Dungeon"]);
        assert_eq!(base.artists, ["Chugong"]);
        assert_eq!(base.description.as_deref(), Some("From AniList"));
        assert_eq!(base.links, [("anilist".to_string(), "1".to_string())]);

        let p = &base.provenance;
        assert_eq!(p["title.english"], ["mangadex"]);
        assert_eq!(p["title.japanese"], ["anilist"]);
        assert_eq!(p["tags"], ["mangadex", "anilist"]);
        assert_eq!(p["artists"], ["mangadex"]);
        assert_eq!(p["description"], ["anilist"]);
        assert_eq!(p["url.anilist"], ["https://anilist.co/manga/1"]);

        // Merging again changes nothing.
        let snapshot = base.clone();
        merge(&mut base, &extra);
        assert_eq!(base, snapshot);
    }

    #[test]
    fn cleans_markdown_and_html() {
        let md = "A **bold** [story](https://x.y/z) here.\n\n\n\nSecond.\n\n---\n**Links:**\n- [Raw](https://r)";
        assert_eq!(clean_description(md), "A bold story here.\n\nSecond.");
        let al = "Line one.\n<br><br>\n(Source: Tappytoon)\n<i>Note</i> [not a link";
        assert_eq!(
            clean_description(al),
            "Line one.\n\n\n(Source: Tappytoon)\nNote [not a link".replace("\n\n\n", "\n\n")
        );
    }
}
