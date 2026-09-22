//! Content delivery descriptors - channel-agnostic rich content with multiple
//! representations.
//!
//! A single piece of content (e.g. "月票") carries several optional
//! representations (`text`, `link`, `image`, `video`, `file`, `miniprogram`).
//! Each channel's [`Channel::deliver`](crate::channel::Channel::deliver) picks
//! the highest-fidelity form it supports; everything degrades to `text`.
//!
//! This lets one content key be delivered as a miniprogram card on a 服务号,
//! a file/video on wecom kf, and a plain link on iLink - same content, best
//! form per channel.

use serde::{Deserialize, Serialize};

use crate::error::{CarrierError, CarrierResult};

/// A media reference, resolved from any of: a public `url`, a local `file_path`,
/// or a pre-uploaded platform `media_id`. Channels use whichever they can -
/// `media_id` is only valid on the platform that issued it.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MediaRef {
    /// Public URL the channel can fetch, or pass directly for URL-based media.
    pub url: Option<String>,
    /// Local file path (absolute, or relative to `~/.aginx/carrier`).
    pub file_path: Option<String>,
    /// Pre-uploaded platform media_id (only valid where it was issued).
    pub media_id: Option<String>,
}

impl MediaRef {
    pub fn is_empty(&self) -> bool {
        self.url.is_none() && self.file_path.is_none() && self.media_id.is_none()
    }
}

/// A link card: title + description + click URL + optional cover image URL.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LinkContent {
    pub title: String,
    pub desc: String,
    pub url: String,
    /// Cover image URL (public; no upload needed).
    pub pic_url: Option<String>,
}

/// A miniprogram card.
///
/// Thumb handling differs per channel: weixin-oa accepts an OA permanent
/// `thumb_media_id` directly; wecom kf uses a *separate* media library and
/// must re-upload from `thumb_url`/`thumb_file` (an OA media_id is invalid
/// there). Descriptors therefore carry both; each channel picks what it can use.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MiniprogramContent {
    pub appid: String,
    pub pagepath: String,
    pub title: String,
    /// OA permanent thumb_media_id - only usable on weixin-oa.
    pub thumb_media_id: Option<String>,
    /// Public cover URL - downloaded + re-uploaded per channel (wecom kf).
    pub thumb_url: Option<String>,
    /// Local cover file - uploaded per channel.
    pub thumb_file: Option<String>,
}

/// A template message (WeChat OA template/send; other channels degrade to
/// `as_text`). Carries no 48h-window limit on weixin-oa, which makes it the
/// carrier for scheduled pushes to followers outside the customer-service
/// window.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TemplateContent {
    /// Template id from the account's template library.
    pub template_id: String,
    /// Per-template field map, e.g. `{"thing1": {"value": "..."}}`. Kept as
    /// raw JSON because each template's field layout is account-specific.
    pub data: serde_json::Value,
    /// Optional jump URL (mutually exclusive with `miniprogram` on WeChat).
    pub url: Option<String>,
    /// Optional miniprogram jump target: `{"appid": "...", "pagepath": "..."}`.
    pub miniprogram: Option<serde_json::Value>,
}

impl TemplateContent {
    /// Whether this has the minimum the weixin-oa renderer needs (template_id
    /// non-empty). Data may legitimately be `{}` for templates with no fields.
    pub fn is_complete(&self) -> bool {
        !self.template_id.is_empty()
    }
}

/// Channel-agnostic rich content. Each representation is optional; a channel's
/// `deliver()` picks the best one it supports and degrades to `text` otherwise.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ContentDescriptor {
    /// Plain text - the universal lowest-fidelity fallback.
    pub text: Option<String>,
    pub link: Option<LinkContent>,
    pub image: Option<MediaRef>,
    pub video: Option<MediaRef>,
    pub file: Option<MediaRef>,
    pub voice: Option<MediaRef>,
    pub miniprogram: Option<MiniprogramContent>,
    pub template: Option<TemplateContent>,
}

impl MiniprogramContent {
    /// Whether this card has the fields a channel needs to actually render it
    /// (appid + pagepath + title). Thumb is resolved per-channel, so it is not
    /// required here. Partial overrides that leave a required field empty should
    /// be treated as "no miniprogram representation" so the channel degrades to
    /// the next form instead of sending a broken card.
    pub fn is_complete(&self) -> bool {
        !self.appid.is_empty() && !self.pagepath.is_empty() && !self.title.is_empty()
    }
}

impl ContentDescriptor {
    /// The best plain-text rendering (text itself, or a formatted link), for
    /// channels that can only send text (the default `deliver` fallback).
    pub fn as_text(&self) -> Option<String> {
        if let Some(t) = &self.text {
            return Some(t.clone());
        }
        if let Some(l) = &self.link {
            if l.title.is_empty() {
                return Some(l.url.clone());
            }
            return Some(format!("{}\n{}", l.title, l.url));
        }
        None
    }

    /// Apply one `field=value` override. `field` supports dotted paths such as
    /// `miniprogram.appid`. Returns an error for unknown fields so callers can
    /// surface typos (e.g. `miniprogram.app_id` vs `miniprogram.appid`) instead
    /// of silently dropping the override and sending the wrong content.
    pub fn apply_override(&mut self, field: &str, value: &str) -> CarrierResult<()> {
        match field {
            "text" => self.text = Some(value.to_string()),
            "link.title" => {
                self.link.get_or_insert_with(LinkContent::default).title = value.to_string();
            }
            "link.desc" => {
                self.link.get_or_insert_with(LinkContent::default).desc = value.to_string();
            }
            "link.url" => {
                self.link.get_or_insert_with(LinkContent::default).url = value.to_string();
            }
            "link.pic_url" => {
                self.link.get_or_insert_with(LinkContent::default).pic_url =
                    Some(value.to_string());
            }
            "image.url" => {
                self.image.get_or_insert_with(MediaRef::default).url = Some(value.to_string());
            }
            "image.file_path" => {
                self.image.get_or_insert_with(MediaRef::default).file_path =
                    Some(value.to_string());
            }
            "image.media_id" => {
                self.image.get_or_insert_with(MediaRef::default).media_id = Some(value.to_string());
            }
            "video.url" => {
                self.video.get_or_insert_with(MediaRef::default).url = Some(value.to_string());
            }
            "video.file_path" => {
                self.video.get_or_insert_with(MediaRef::default).file_path =
                    Some(value.to_string());
            }
            "video.media_id" => {
                self.video.get_or_insert_with(MediaRef::default).media_id = Some(value.to_string());
            }
            "file.url" => {
                self.file.get_or_insert_with(MediaRef::default).url = Some(value.to_string());
            }
            "file.file_path" => {
                self.file.get_or_insert_with(MediaRef::default).file_path = Some(value.to_string());
            }
            "file.media_id" => {
                self.file.get_or_insert_with(MediaRef::default).media_id = Some(value.to_string());
            }
            "voice.url" => {
                self.voice.get_or_insert_with(MediaRef::default).url = Some(value.to_string());
            }
            "voice.file_path" => {
                self.voice.get_or_insert_with(MediaRef::default).file_path =
                    Some(value.to_string());
            }
            "voice.media_id" => {
                self.voice.get_or_insert_with(MediaRef::default).media_id = Some(value.to_string());
            }
            "miniprogram.appid" => {
                self.miniprogram
                    .get_or_insert_with(MiniprogramContent::default)
                    .appid = value.to_string();
            }
            "miniprogram.pagepath" => {
                self.miniprogram
                    .get_or_insert_with(MiniprogramContent::default)
                    .pagepath = value.to_string();
            }
            "miniprogram.title" => {
                self.miniprogram
                    .get_or_insert_with(MiniprogramContent::default)
                    .title = value.to_string();
            }
            "miniprogram.thumb_media_id" => {
                self.miniprogram
                    .get_or_insert_with(MiniprogramContent::default)
                    .thumb_media_id = Some(value.to_string());
            }
            "miniprogram.thumb_url" => {
                self.miniprogram
                    .get_or_insert_with(MiniprogramContent::default)
                    .thumb_url = Some(value.to_string());
            }
            "miniprogram.thumb_file" => {
                self.miniprogram
                    .get_or_insert_with(MiniprogramContent::default)
                    .thumb_file = Some(value.to_string());
            }
            "template.template_id" => {
                self.template
                    .get_or_insert_with(TemplateContent::default)
                    .template_id = value.to_string();
            }
            "template.url" => {
                self.template
                    .get_or_insert_with(TemplateContent::default)
                    .url = Some(value.to_string());
            }
            // `template.data` is a JSON object, not a scalar — deliberately NOT
            // overridable via `field=value` strings (falls into the unknown-
            // field error arm below on purpose).
            _ => {
                return Err(CarrierError::InvalidInput(format!(
                    "unknown override field: {field}"
                )));
            }
        }
        Ok(())
    }
}

/// One entry in a per-agent content registry: a `key` (what the agent writes in
/// a `[DELIVER:key]` marker) plus the content's representations.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DeliverableEntry {
    /// Content key referenced by `[DELIVER:<key>]` markers.
    pub key: String,
    #[serde(flatten)]
    pub content: ContentDescriptor,
}

/// Per-agent content registry, loaded from
/// `~/.aginx/carrier/workspaces/{agent}/content.toml`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ContentConfig {
    pub deliverables: Vec<DeliverableEntry>,
}

impl ContentConfig {
    /// Look up a content descriptor by key.
    pub fn get(&self, key: &str) -> Option<&ContentDescriptor> {
        self.deliverables
            .iter()
            .find(|d| d.key == key)
            .map(|d| &d.content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_content_toml() {
        let toml = r#"
[[deliverables]]
key = "月票"
text = "这是咱们的月票"
[deliverables.link]
title = "86优团-月票"
url = "https://bus.86xq.com/x"
[deliverables.miniprogram]
appid = "wxabc"
pagepath = "pages/index/type/type?id=883"
title = "86优团-月票"
thumb_media_id = "OA_PERM"
thumb_url = "https://mmecoa.qpic.cn/cover.png"
"#;
        let cfg: ContentConfig = toml::from_str(toml).unwrap();
        let d = cfg.get("月票").expect("月票 entry");
        assert_eq!(d.text.as_deref(), Some("这是咱们的月票"));
        assert_eq!(d.link.as_ref().unwrap().title, "86优团-月票");
        let mp = d.miniprogram.as_ref().unwrap();
        assert_eq!(mp.appid, "wxabc");
        assert_eq!(mp.thumb_media_id.as_deref(), Some("OA_PERM"));
        assert_eq!(
            mp.thumb_url.as_deref(),
            Some("https://mmecoa.qpic.cn/cover.png")
        );
        assert!(cfg.get("不存在").is_none());
    }

    #[test]
    fn as_text_falls_back_to_link() {
        let d = ContentDescriptor {
            link: Some(LinkContent {
                title: "t".into(),
                url: "https://x".into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(d.as_text().unwrap(), "t\nhttps://x");
        let d2 = ContentDescriptor {
            text: Some("plain".into()),
            link: Some(LinkContent {
                title: "t".into(),
                url: "https://x".into(),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(d2.as_text().unwrap(), "plain");
    }

    #[test]
    fn miniprogram_is_complete_requires_core_fields() {
        let full = MiniprogramContent {
            appid: "wx".into(),
            pagepath: "pages/x".into(),
            title: "t".into(),
            ..Default::default()
        };
        assert!(full.is_complete());
        // Missing any of appid/pagepath/title -> not complete (partial override
        // case: must degrade instead of sending a broken card).
        assert!(!MiniprogramContent {
            appid: String::new(),
            pagepath: "p".into(),
            title: "t".into(),
            ..Default::default()
        }
        .is_complete());
        assert!(!MiniprogramContent {
            appid: "wx".into(),
            pagepath: String::new(),
            title: "t".into(),
            ..Default::default()
        }
        .is_complete());
        assert!(!MiniprogramContent {
            appid: "wx".into(),
            pagepath: "p".into(),
            title: String::new(),
            ..Default::default()
        }
        .is_complete());
    }

    #[test]
    fn apply_override_rejects_unknown_field() {
        let mut d = ContentDescriptor::default();
        // Typo: app_id vs appid - must error so the caller skips delivery
        // instead of silently sending a card with an empty appid.
        assert!(d.apply_override("miniprogram.app_id", "wx").is_err());
        assert!(d.miniprogram.is_none());
        // Correct field applies and makes the card complete.
        d.apply_override("miniprogram.appid", "wx").unwrap();
        d.apply_override("miniprogram.pagepath", "pages/x").unwrap();
        d.apply_override("miniprogram.title", "t").unwrap();
        assert!(d.miniprogram.as_ref().unwrap().is_complete());
    }

    /// Template payloads (zero-LLM scheduled pushes beyond the 48h window)
    /// parse standalone and survive JSON round-trips; template.data is raw
    /// JSON, so no scalar override arm exists for it.
    #[test]
    fn template_content_parses_and_overrides() {
        let json = r#"{"template":{"template_id":"tpl-1","data":{"thing1":{"value":"月卡到期"}},"url":"https://x"}}"#;
        let d: ContentDescriptor = serde_json::from_str(json).unwrap();
        let t = d.template.as_ref().unwrap();
        assert_eq!(t.template_id, "tpl-1");
        assert_eq!(t.data["thing1"]["value"], "月卡到期");
        assert!(t.is_complete());

        // Old payloads without template still parse (additive field).
        let old: ContentDescriptor = serde_json::from_str(r#"{"text":"hi"}"#).unwrap();
        assert!(old.template.is_none());

        // Scalar overrides cover template_id/url; data is deliberately not a
        // string override (JSON object) and must error instead of half-applying.
        let mut d2 = ContentDescriptor::default();
        d2.apply_override("template.template_id", "tpl-2").unwrap();
        d2.apply_override("template.url", "https://y").unwrap();
        assert!(d2.apply_override("template.data", "{}").is_err());
        assert!(d2.template.as_ref().unwrap().is_complete());
        assert_eq!(
            d2.template.as_ref().unwrap().url.as_deref(),
            Some("https://y")
        );

        // Round-trip through the full descriptor.
        let rt: ContentDescriptor =
            serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
        assert_eq!(rt.template.unwrap().template_id, "tpl-1");
    }
}
