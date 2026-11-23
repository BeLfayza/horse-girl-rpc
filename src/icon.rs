use crate::hachimi;
use crate::telemetry::CharacterIconIdentity;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

const WIKI_API: &str = "https://umamusu.wiki/w/api.php";
const WIKI_IMAGE_PREFIX: &str = "https://umamusu.wiki/w/";
const HTTP_TIMEOUT: Duration = Duration::from_secs(10);
const RETRY_DELAY: Duration = Duration::from_secs(10 * 60);
const MAX_DISCORD_IMAGE_LENGTH: usize = 300;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolvedIcon {
    pub url: String,
    pub card_id: i32,
}

enum CacheEntry {
    Pending,
    Ready(ResolvedIcon),
    Missing,
    Failed { retry_at: Instant },
}

struct IconService {
    sender: Sender<CharacterIconIdentity>,
    cache: Arc<Mutex<HashMap<CharacterIconIdentity, CacheEntry>>>,
}

static SERVICE: OnceLock<IconService> = OnceLock::new();

pub(crate) fn start() -> Result<(), String> {
    let (sender, receiver) = mpsc::channel();
    let cache = Arc::new(Mutex::new(HashMap::new()));
    let worker_cache = Arc::clone(&cache);

    thread::Builder::new()
        .name("HorseGirlIconResolver".into())
        .spawn(move || resolver_loop(receiver, worker_cache))
        .map_err(|error| format!("failed to start character icon resolver: {error}"))?;

    SERVICE
        .set(IconService { sender, cache })
        .map_err(|_| "character icon resolver is already initialized".to_owned())
}

pub(crate) fn get_or_request(identity: CharacterIconIdentity) -> Option<ResolvedIcon> {
    let service = SERVICE.get()?;

    {
        let mut cache = lock_cache(&service.cache);
        match cache.get_mut(&identity) {
            Some(CacheEntry::Ready(icon)) => return Some(icon.clone()),
            Some(CacheEntry::Pending | CacheEntry::Missing) => return None,
            Some(CacheEntry::Failed { retry_at }) if Instant::now() < *retry_at => return None,
            Some(entry @ CacheEntry::Failed { .. }) => {
                *entry = CacheEntry::Pending;
            }
            None => {
                cache.insert(identity, CacheEntry::Pending);
            }
        }
    }

    if service.sender.send(identity).is_err() {
        lock_cache(&service.cache).insert(
            identity,
            CacheEntry::Failed {
                retry_at: Instant::now() + RETRY_DELAY,
            },
        );
    }

    None
}

fn resolver_loop(
    receiver: Receiver<CharacterIconIdentity>,
    cache: Arc<Mutex<HashMap<CharacterIconIdentity, CacheEntry>>>,
) {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(HTTP_TIMEOUT))
        .build();
    let agent: ureq::Agent = config.into();

    while let Ok(identity) = receiver.recv() {
        match resolve_icon(&agent, identity) {
            Ok(Some(icon)) => {
                hachimi::log_info(format!("Resolved character icon for card {}", icon.card_id));
                lock_cache(&cache).insert(identity, CacheEntry::Ready(icon));
            }
            Ok(None) => {
                hachimi::log_warn(format!(
                    "No Umamusume Wiki icon found for card candidates {:?}",
                    candidate_card_ids(identity)
                ));
                lock_cache(&cache).insert(identity, CacheEntry::Missing);
            }
            Err(error) => {
                hachimi::log_warn(format!(
                    "Character icon lookup failed; text RPC remains active: {error}"
                ));
                lock_cache(&cache).insert(
                    identity,
                    CacheEntry::Failed {
                        retry_at: Instant::now() + RETRY_DELAY,
                    },
                );
            }
        }
    }
}

fn resolve_icon(
    agent: &ureq::Agent,
    identity: CharacterIconIdentity,
) -> Result<Option<ResolvedIcon>, String> {
    resolve_from_candidates(identity, |card_id| resolve_wiki_image(agent, card_id))
}

fn resolve_from_candidates<F>(
    identity: CharacterIconIdentity,
    mut resolve: F,
) -> Result<Option<ResolvedIcon>, String>
where
    F: FnMut(i32) -> Result<Option<String>, String>,
{
    for card_id in candidate_card_ids(identity) {
        if let Some(url) = resolve(card_id)? {
            return Ok(Some(ResolvedIcon { url, card_id }));
        }
    }

    Ok(None)
}

fn resolve_wiki_image(agent: &ureq::Agent, card_id: i32) -> Result<Option<String>, String> {
    let request_url = wiki_api_url(card_id);
    let mut response = agent
        .get(&request_url)
        .header("User-Agent", "HorseGirlRPC/0.1")
        .call()
        .map_err(|error| format!("MediaWiki request for card {card_id} failed: {error}"))?;
    let response = response
        .body_mut()
        .read_json::<WikiResponse>()
        .map_err(|error| format!("invalid MediaWiki response for card {card_id}: {error}"))?;

    extract_image_url(response)
}

fn candidate_card_ids(identity: CharacterIconIdentity) -> Vec<i32> {
    let mut candidates = Vec::with_capacity(3);
    for card_id in [
        identity.current_card_id,
        identity.selected_card_id,
        identity.base_card_id,
    ] {
        if card_id > 0 && !candidates.contains(&card_id) {
            candidates.push(card_id);
        }
    }
    candidates
}

fn wiki_api_url(card_id: i32) -> String {
    format!(
        "{WIKI_API}?action=query&prop=imageinfo&iiprop=url%7Cmime%7Csize&format=json&formatversion=2&titles=File%3AGame_Playable_Icon_{card_id}.png"
    )
}

fn extract_image_url(response: WikiResponse) -> Result<Option<String>, String> {
    let Some(image) = response
        .query
        .pages
        .into_iter()
        .find_map(|page| page.imageinfo.and_then(|images| images.into_iter().next()))
    else {
        return Ok(None);
    };

    if image.mime != "image/png" || image.width == 0 || image.height == 0 {
        return Err("MediaWiki returned an invalid character image".to_owned());
    }
    if !image.url.starts_with(WIKI_IMAGE_PREFIX) {
        return Err("MediaWiki returned an unexpected image host".to_owned());
    }
    if image.url.len() > MAX_DISCORD_IMAGE_LENGTH {
        return Err("MediaWiki image URL exceeds Discord's length limit".to_owned());
    }

    Ok(Some(image.url))
}

fn lock_cache(
    cache: &Mutex<HashMap<CharacterIconIdentity, CacheEntry>>,
) -> MutexGuard<'_, HashMap<CharacterIconIdentity, CacheEntry>> {
    cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Debug, Deserialize)]
struct WikiResponse {
    query: WikiQuery,
}

#[derive(Debug, Deserialize)]
struct WikiQuery {
    pages: Vec<WikiPage>,
}

#[derive(Debug, Deserialize)]
struct WikiPage {
    imageinfo: Option<Vec<WikiImageInfo>>,
}

#[derive(Debug, Deserialize)]
struct WikiImageInfo {
    url: String,
    mime: String,
    width: u32,
    height: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn costume_then_selected_then_base_candidates_are_unique() {
        let identity = CharacterIconIdentity {
            character_id: 1065,
            current_card_id: 106502,
            selected_card_id: 106502,
            base_card_id: 106501,
        };

        assert_eq!(candidate_card_ids(identity), vec![106502, 106501]);
    }

    #[test]
    fn missing_costume_icon_falls_back_to_base_card() {
        let identity = CharacterIconIdentity {
            character_id: 1001,
            current_card_id: 100103,
            selected_card_id: 100103,
            base_card_id: 100101,
        };
        let resolved = resolve_from_candidates(identity, |card_id| {
            Ok((card_id == 100101).then(|| {
                "https://umamusu.wiki/w/images/1/1c/Game_Playable_Icon_100101.png".to_owned()
            }))
        })
        .unwrap()
        .expect("base icon should resolve");

        assert_eq!(resolved.card_id, 100101);
    }

    #[test]
    fn parses_mediawiki_direct_image_url() {
        let response: WikiResponse = serde_json::from_str(
            r#"{
                "query": {
                    "pages": [{
                        "imageinfo": [{
                            "url": "https://umamusu.wiki/w/images/8/83/Game_Playable_Icon_106502.png",
                            "mime": "image/png",
                            "width": 256,
                            "height": 284
                        }]
                    }]
                }
            }"#,
        )
        .expect("fixture should parse");

        assert_eq!(
            extract_image_url(response).expect("image should be valid"),
            Some("https://umamusu.wiki/w/images/8/83/Game_Playable_Icon_106502.png".to_owned())
        );
    }

    #[test]
    fn missing_mediawiki_file_is_not_an_error() {
        let response: WikiResponse = serde_json::from_str(
            r#"{
                "query": {
                    "pages": [{ "missing": true }]
                }
            }"#,
        )
        .expect("fixture should parse");

        assert_eq!(extract_image_url(response).unwrap(), None);
    }
}
