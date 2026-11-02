use anyhow::{Context, Result};
use serde_json::Value;

use crate::emotes::EmoteInfo;

const SEVENTV_API: &str = "https://7tv.io/v3";

pub async fn fetch_channel(client: &reqwest::Client, twitch_id: &str) -> Result<Vec<EmoteInfo>> {
    let url = format!("{SEVENTV_API}/users/twitch/{twitch_id}");
    let body: Value = client
        .get(url)
        .send()
        .await
        .context("7TV: запрос сета канала")?
        .json()
        .await
        .context("7TV: разбор ответа канала")?;
    Ok(parse_set(&body["emote_set"]))
}

pub async fn fetch_global(client: &reqwest::Client) -> Result<Vec<EmoteInfo>> {
    let url = format!("{SEVENTV_API}/emote-sets/global");
    let body: Value = client
        .get(url)
        .send()
        .await
        .context("7TV: запрос глобального сета")?
        .json()
        .await
        .context("7TV: разбор глобального сета")?;
    Ok(parse_set(&body))
}

pub fn parse_set(set: &Value) -> Vec<EmoteInfo> {
    let mut result = Vec::new();
    for emote in set["emotes"].as_array().into_iter().flatten() {
        let Some(name) = emote["name"].as_str() else {
            continue;
        };
        let host = emote["data"]["host"]["url"].as_str().unwrap_or_default();
        let files = &emote["data"]["host"]["files"];
        let Some((file, width, height)) = pick_file_height(files, 64) else {
            continue;
        };
        if host.is_empty() {
            continue;
        }
        result.push(EmoteInfo {
            name: name.to_string(),
            uri: format!("https:{host}/{file}"),
            aspect: if height > 0 {
                width as f32 / height as f32
            } else {
                1.0
            },
            zero_width: emote["flags"].as_u64().unwrap_or(0) & 1 == 1,
        });
    }
    result
}

/// Picks the smallest file whose height is at least `need_px`; falls back to
/// the tallest one.
pub fn pick_file_height(files: &Value, need_px: u32) -> Option<(String, u32, u32)> {
    let files = files.as_array()?;
    let mut best: Option<(String, u32, u32)> = None;
    for file in files {
        let Some(name) = file["name"].as_str() else {
            continue;
        };
        let width = file["width"].as_u64().unwrap_or(0) as u32;
        let height = file["height"].as_u64().unwrap_or(0) as u32;
        if height == 0 {
            continue;
        }
        match &best {
            Some((_, _, best_height)) => {
                let better = if *best_height < need_px {
                    height > *best_height
                } else {
                    height >= need_px && height < *best_height
                };
                if better {
                    best = Some((name.to_string(), width, height));
                }
            }
            None => best = Some((name.to_string(), width, height)),
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Value {
        serde_json::from_str(include_str!("fixtures/seventv_set.json")).unwrap()
    }

    #[test]
    fn parses_set_with_zero_width_and_aspect() {
        let emotes = parse_set(&fixture());
        let wide = emotes.iter().find(|e| e.name == "privet").unwrap();
        assert_eq!(wide.aspect, 3.0);
        assert!(wide.uri.starts_with("https://cdn.7tv.app/"));
        assert!(wide.uri.ends_with("/2x.webp"));
        let zero_width = emotes.iter().find(|e| e.name == "RainTime").unwrap();
        assert!(zero_width.zero_width);
    }

    #[test]
    fn picks_scale_closest_above_needed_height() {
        let v = fixture();
        let files = &v["emotes"][0]["data"]["host"]["files"];
        let (url, width, height) = pick_file_height(files, 40).unwrap();
        assert_eq!(height, 64);
        assert_eq!(width, 192);
        assert!(url.ends_with(".webp"));
    }

    #[test]
    fn merges_channel_over_global() {
        let global = vec![EmoteInfo {
            name: "x".into(),
            uri: "global".into(),
            aspect: 1.0,
            zero_width: false,
        }];
        let channel = vec![EmoteInfo {
            name: "x".into(),
            uri: "channel".into(),
            aspect: 1.0,
            zero_width: false,
        }];
        let catalog = crate::emotes::EmoteCatalog::merged(channel, global);
        assert_eq!(catalog.get("x").unwrap().uri, "channel");
    }
}
