//! Live end-to-end check of cross-source enrichment: MangaDex details -> AniList (via the
//! work's `links`) -> merge with provenance. Hits the network, so it is ignored by default:
//! `cargo test --test enrichment_live -- --ignored`

use kumadoujin::core::merge;
use kumadoujin::sources::adapters::{anilist::AniList, mangadex::MangaDex};
use kumadoujin::sources::traits::CatalogProvider;

#[tokio::test]
#[ignore]
async fn mangadex_details_enriched_from_anilist() {
    let md = MangaDex::new().unwrap();
    let al = AniList::new().unwrap();

    // Solo Leveling: MangaDex links it to AniList 105398.
    let mut m = md
        .details("32d76d19-8a05-4db0-9fc2-e0b0648fe9d0")
        .await
        .unwrap();
    let (_, al_id) = m
        .links
        .iter()
        .find(|(s, _)| s == "anilist")
        .cloned()
        .unwrap();
    assert_eq!(al_id, "105398");

    let before_tags = m.tags.len();
    merge::record_primary(&mut m);
    merge::merge(&mut m, &al.details(&al_id).await.unwrap());

    assert!(m.tags.len() > before_tags, "AniList adds tags");
    assert_eq!(m.provenance["title.english"], ["mangadex"]);
    assert!(m.provenance["tags"].contains(&"anilist".to_string()));
    assert!(m.provenance.contains_key("url.anilist"));
}
