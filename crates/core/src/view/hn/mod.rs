use crate::color::{BLACK, SEPARATOR_NORMAL, TEXT_INVERTED_HARD, TEXT_NORMAL, WHITE};
use crate::context::Context;
use crate::device::CURRENT_DEVICE;
use crate::document::html::dom::NodeData;
use crate::document::html::xml::XmlParser;
use crate::font::{font_from_style, Fonts, MD_AUTHOR, MD_TITLE, NORMAL_STYLE};
use crate::framebuffer::{Framebuffer, UpdateMode};
use crate::geom::{halves, CycleDir, Dir, Rectangle};
use crate::gesture::GestureEvent;
use crate::helpers::decode_entities;
use crate::input::DeviceEvent;
use crate::unit::scale_by_dpi;
use crate::view::common::{toggle_battery_menu, toggle_clock_menu, toggle_main_menu};
use crate::view::filler::Filler;
use crate::view::icon::Icon;
use crate::view::label::Label;
use crate::view::menu::{Menu, MenuKind};
use crate::view::top_bar::TopBar;
use crate::view::{Align, Bus, EntryId, EntryKind, Event, Hub, Id, ID_FEEDER};
use crate::view::{
    RenderData, RenderQueue, View, ViewId, BIG_BAR_HEIGHT, SMALL_BAR_HEIGHT, THICKNESS_MEDIUM,
};
use chrono::Utc;
use reqwest::blocking::{Client, Response};
use reqwest::Certificate;
use serde::de::{DeserializeOwned, Deserializer};
use serde::Deserialize;
use serde_json::Value as JsonValue;
use std::collections::HashSet;
use std::error::Error as StdError;
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::Duration;

pub const ALGOLIA_HN_API: &str = "https://hn.algolia.com";
pub const HCKER_NEWS_API: &str = "https://hcker.news";
pub const COMMENT_COLLAPSE_PREFIX: &str = "hn-collapse:";
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_COMMENT_INDENT: usize = 6;
const MAX_STORIES: usize = 50;

#[derive(Debug, Copy, Clone, Hash, Eq, PartialEq)]
pub enum TimeWindow {
    Day,
    Week,
    Month,
}

impl TimeWindow {
    pub const ALL: [TimeWindow; 3] = [TimeWindow::Day, TimeWindow::Week, TimeWindow::Month];

    pub const fn label(self) -> &'static str {
        match self {
            TimeWindow::Day => "Day",
            TimeWindow::Week => "Week",
            TimeWindow::Month => "Month",
        }
    }

    pub const fn index(self) -> usize {
        match self {
            TimeWindow::Day => 0,
            TimeWindow::Week => 1,
            TimeWindow::Month => 2,
        }
    }

    pub const fn tab_index(self) -> usize {
        self.index()
    }

    pub const fn duration_label(self) -> &'static str {
        match self {
            TimeWindow::Day => "24h",
            TimeWindow::Week => "7d",
            TimeWindow::Month => "30d",
        }
    }

    pub const fn duration_seconds(self) -> i64 {
        match self {
            TimeWindow::Day => 24 * 60 * 60,
            TimeWindow::Week => 7 * 24 * 60 * 60,
            TimeWindow::Month => 30 * 24 * 60 * 60,
        }
    }

    pub const fn duration(self) -> Duration {
        Duration::from_secs(self.duration_seconds() as u64)
    }

    pub const fn cutoff(self, now: i64) -> i64 {
        now.saturating_sub(self.duration_seconds())
    }
}

fn string_or_default<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    let value = JsonValue::deserialize(deserializer)?;
    Ok(match value {
        JsonValue::String(value) => value,
        JsonValue::Number(value) => value.to_string(),
        JsonValue::Bool(value) => value.to_string(),
        _ => String::new(),
    })
}

fn u64_or_default<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let value = JsonValue::deserialize(deserializer)?;
    Ok(match value {
        JsonValue::Number(value) => value.as_u64().unwrap_or_default(),
        JsonValue::String(value) => value.parse().unwrap_or_default(),
        _ => 0,
    })
}

fn i64_or_default<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: Deserializer<'de>,
{
    let value = JsonValue::deserialize(deserializer)?;
    Ok(match value {
        JsonValue::Number(value) => value.as_i64().unwrap_or_default(),
        JsonValue::String(value) => value.parse().unwrap_or_default(),
        _ => 0,
    })
}

fn timestamp_or_default<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: Deserializer<'de>,
{
    let value = JsonValue::deserialize(deserializer)?;
    Ok(match value {
        JsonValue::Number(value) => value.as_i64().unwrap_or_default(),
        JsonValue::String(value) => value
            .parse()
            .ok()
            .or_else(|| {
                chrono::DateTime::parse_from_rfc3339(&value)
                    .ok()
                    .map(|timestamp| timestamp.timestamp())
            })
            .unwrap_or_default(),
        _ => 0,
    })
}

fn bool_or_default<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    let value = JsonValue::deserialize(deserializer)?;
    Ok(value.as_bool().unwrap_or(false))
}

fn hits_or_default<'de, D>(deserializer: D) -> Result<Vec<HnHit>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = JsonValue::deserialize(deserializer)?;
    Ok(value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|value| serde_json::from_value(value.clone()).ok())
        .collect())
}

fn children_or_default<'de, D>(deserializer: D) -> Result<Vec<HnItem>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = JsonValue::deserialize(deserializer)?;
    Ok(value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|value| serde_json::from_value(value.clone()).ok())
        .collect())
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct HnHit {
    #[serde(rename = "objectID", deserialize_with = "string_or_default")]
    pub object_id: String,
    #[serde(deserialize_with = "string_or_default")]
    pub title: String,
    #[serde(deserialize_with = "string_or_default")]
    pub url: String,
    #[serde(deserialize_with = "string_or_default")]
    pub author: String,
    #[serde(deserialize_with = "u64_or_default")]
    pub points: u64,
    #[serde(rename = "num_comments", deserialize_with = "u64_or_default")]
    pub num_comments: u64,
    #[serde(rename = "created_at_i", deserialize_with = "i64_or_default")]
    pub created_at_i: i64,
    #[serde(rename = "story_text", deserialize_with = "string_or_default")]
    pub story_text: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct HnSearchResult {
    #[serde(deserialize_with = "hits_or_default")]
    pub hits: Vec<HnHit>,
    #[serde(rename = "nbHits", deserialize_with = "usize_or_default")]
    pub nb_hits: usize,
    #[serde(deserialize_with = "usize_or_default")]
    pub page: usize,
    #[serde(rename = "nbPages", deserialize_with = "usize_or_default")]
    pub nb_pages: usize,
    #[serde(rename = "hitsPerPage", deserialize_with = "usize_or_default")]
    pub hits_per_page: usize,
}

fn usize_or_default<'de, D>(deserializer: D) -> Result<usize, D::Error>
where
    D: Deserializer<'de>,
{
    let value = JsonValue::deserialize(deserializer)?;
    Ok(match value {
        JsonValue::Number(value) => value
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or_default(),
        JsonValue::String(value) => value.parse().unwrap_or_default(),
        _ => 0,
    })
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct HnItem {
    #[serde(alias = "objectID", deserialize_with = "string_or_default")]
    pub id: String,
    #[serde(rename = "type", deserialize_with = "string_or_default")]
    pub item_type: String,
    #[serde(deserialize_with = "string_or_default")]
    pub title: String,
    #[serde(deserialize_with = "string_or_default")]
    pub url: String,
    #[serde(deserialize_with = "string_or_default")]
    pub author: String,
    #[serde(deserialize_with = "u64_or_default")]
    pub points: u64,
    #[serde(rename = "num_comments", deserialize_with = "u64_or_default")]
    pub num_comments: u64,
    #[serde(alias = "createdAt", deserialize_with = "timestamp_or_default")]
    pub created_at_i: i64,
    #[serde(alias = "rawHtml", deserialize_with = "string_or_default")]
    pub text: String,
    #[serde(rename = "story_text", deserialize_with = "string_or_default")]
    pub story_text: String,
    #[serde(deserialize_with = "children_or_default")]
    pub children: Vec<HnItem>,
    #[serde(alias = "isDeleted", deserialize_with = "bool_or_default")]
    pub deleted: bool,
    #[serde(deserialize_with = "bool_or_default")]
    pub dead: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct HnCommentsResponse {
    #[serde(rename = "storyId", deserialize_with = "string_or_default")]
    story_id: String,
    #[serde(rename = "storyTitle", deserialize_with = "string_or_default")]
    story_title: String,
    #[serde(rename = "storyUrl", deserialize_with = "string_or_default")]
    story_url: String,
    #[serde(rename = "storyAuthor", deserialize_with = "string_or_default")]
    story_author: String,
    #[serde(rename = "storyScore", deserialize_with = "u64_or_default")]
    story_score: u64,
    #[serde(rename = "storyTime", deserialize_with = "timestamp_or_default")]
    story_time: i64,
    #[serde(deserialize_with = "children_or_default")]
    comments: Vec<HnItem>,
    #[serde(rename = "totalCount", deserialize_with = "u64_or_default")]
    total_count: u64,
}

impl HnCommentsResponse {
    fn into_item(self, requested_story_id: &str) -> HnItem {
        HnItem {
            id: if self.story_id.is_empty() {
                requested_story_id.to_string()
            } else {
                self.story_id
            },
            item_type: "story".to_string(),
            title: self.story_title,
            url: self.story_url,
            author: self.story_author,
            points: self.story_score,
            num_comments: self.total_count,
            created_at_i: self.story_time,
            children: self.comments,
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone)]
pub enum HnResponse {
    Stories {
        request_id: u64,
        window: TimeWindow,
        page: usize,
        cutoff: i64,
        result: HnSearchResult,
    },
    Item {
        request_id: u64,
        story_id: String,
        item: HnItem,
    },
    Error {
        request_id: u64,
        window: Option<TimeWindow>,
        page: Option<usize>,
        story_id: Option<String>,
        message: String,
    },
}

fn error_chain(error: &dyn StdError) -> String {
    let mut messages = vec![error.to_string()];
    let mut source = error.source();
    while let Some(error) = source {
        messages.push(error.to_string());
        source = error.source();
    }
    messages.dedup();
    messages.join(": ")
}

struct Api {
    client: Client,
    domain: String,
    comments_domain: String,
}

impl Api {
    fn new(domain: String) -> Result<Api, String> {
        Self::new_with_comments_domain(domain, HCKER_NEWS_API.to_string())
    }

    fn new_with_comments_domain(domain: String, comments_domain: String) -> Result<Api, String> {
        let domain = domain.trim().trim_end_matches('/');
        let domain = if domain.contains("://") {
            domain.to_string()
        } else {
            format!("https://{}", domain)
        };
        let comments_domain = comments_domain.trim().trim_end_matches('/');
        let comments_domain = if comments_domain.contains("://") {
            comments_domain.to_string()
        } else {
            format!("https://{}", comments_domain)
        };
        let roots = webpki_root_certs::TLS_SERVER_ROOT_CERTS
            .iter()
            .map(|der| Certificate::from_der(der.as_ref()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| {
                let message = format!("can't load bundled TLS roots: {}", error_chain(&e));
                eprintln!("[hacker-news] {}.", message);
                message
            })?;
        let client = Client::builder()
            .timeout(HTTP_TIMEOUT)
            .tls_certs_only(roots)
            .build()
            .map_err(|e| {
                let message = format!("can't build HTTP client: {}", error_chain(&e));
                eprintln!("[hacker-news] {} (debug={:?}).", message, e);
                message
            })?;
        Ok(Api {
            client,
            domain,
            comments_domain,
        })
    }

    fn checked_json<T: DeserializeOwned>(operation: &str, response: Response) -> Result<T, String> {
        let status = response.status();
        if status.is_success() {
            response.json().map_err(|e| {
                let message = format!("invalid JSON response: {}", error_chain(&e));
                eprintln!("[hacker-news] {} failed: {}.", operation, message);
                message
            })
        } else {
            let reason = status.canonical_reason().unwrap_or("HTTP error");
            let body = response.text().unwrap_or_default();
            let message = serde_json::from_str::<JsonValue>(&body)
                .ok()
                .and_then(|body| {
                    body.get("message")
                        .or_else(|| body.get("error"))
                        .and_then(JsonValue::as_str)
                        .map(str::to_string)
                })
                .unwrap_or_else(|| format!("{} {}", status.as_u16(), reason));
            eprintln!("[hacker-news] {} failed: {}.", operation, message);
            Err(message)
        }
    }

    fn transport_error(operation: &str, error: reqwest::Error) -> String {
        let message = format!("{} transport error: {}", operation, error_chain(&error));
        eprintln!("[hacker-news] {} (debug={:?}).", message, error);
        message
    }

    fn search(
        &self,
        page: usize,
        hits_per_page: usize,
        cutoff: i64,
    ) -> Result<HnSearchResult, String> {
        let operation = "GET /api/v1/search";
        let numeric_filters = format!("created_at_i>={}", cutoff);
        let page = page.to_string();
        let hits_per_page = hits_per_page.to_string();
        let response = self
            .client
            .get(format!("{}/api/v1/search", self.domain))
            .query(&[
                ("tags", "story"),
                ("numericFilters", numeric_filters.as_str()),
                ("page", page.as_str()),
                ("hitsPerPage", hits_per_page.as_str()),
            ])
            .send()
            .map_err(|e| Self::transport_error(operation, e))?;
        Self::checked_json(operation, response)
    }

    fn comments(&self, story_id: &str) -> Result<HnItem, String> {
        let id = validate_story_id(story_id)?;
        let operation = format!("GET /api/comments/{}", id);
        let response = self
            .client
            .get(format!("{}/api/comments/{}", self.comments_domain, id))
            .header("Accept", "application/json")
            .header("Referer", format!("{}/", self.comments_domain))
            .send()
            .map_err(|e| Self::transport_error(&operation, e))?;
        let result: HnCommentsResponse = Self::checked_json(&operation, response)?;
        Ok(result.into_item(&id.to_string()))
    }
}

pub fn validate_story_id(story_id: &str) -> Result<u64, String> {
    if story_id.is_empty() || !story_id.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!("invalid Hacker News story ID {:?}", story_id));
    }
    let id = story_id
        .parse::<u64>()
        .map_err(|_| format!("invalid Hacker News story ID {:?}", story_id))?;
    if id == 0 {
        return Err(format!("invalid Hacker News story ID {:?}", story_id));
    }
    Ok(id)
}

enum ApiCommand {
    ListStories {
        request_id: u64,
        window: TimeWindow,
        page: usize,
        hits_per_page: usize,
        cutoff: i64,
    },
    GetItem {
        request_id: u64,
        story_id: String,
    },
}

impl ApiCommand {
    fn description(&self) -> String {
        match self {
            ApiCommand::ListStories { window, page, .. } => {
                format!("list {} stories (page {})", window.label(), page + 1)
            }
            ApiCommand::GetItem { story_id, .. } => format!("get story {}", story_id),
        }
    }
}

fn spawn_api_worker(domain: String, hub: &Hub) -> Result<Sender<ApiCommand>, String> {
    let (sender, receiver) = mpsc::channel::<ApiCommand>();
    let (ready_sender, ready_receiver) = mpsc::channel();
    let hub = hub.clone();
    thread::spawn(move || {
        let api = match Api::new(domain) {
            Ok(api) => {
                ready_sender.send(Ok(())).ok();
                api
            }
            Err(message) => {
                ready_sender.send(Err(message)).ok();
                return;
            }
        };
        while let Ok(command) = receiver.recv() {
            let description = command.description();
            let response = match command {
                ApiCommand::ListStories {
                    request_id,
                    window,
                    page,
                    hits_per_page,
                    cutoff,
                } => match api.search(page, hits_per_page, cutoff) {
                    Ok(result) => HnResponse::Stories {
                        request_id,
                        window,
                        page,
                        cutoff,
                        result,
                    },
                    Err(message) => HnResponse::Error {
                        request_id,
                        window: Some(window),
                        page: Some(page),
                        story_id: None,
                        message,
                    },
                },
                ApiCommand::GetItem {
                    request_id,
                    story_id,
                } => match api.comments(&story_id) {
                    Ok(mut item) => {
                        if item.id.is_empty() {
                            item.id = story_id.clone();
                        }
                        HnResponse::Item {
                            request_id,
                            story_id,
                            item,
                        }
                    }
                    Err(message) => HnResponse::Error {
                        request_id,
                        window: None,
                        page: None,
                        story_id: Some(story_id),
                        message,
                    },
                },
            };
            if hub.send(Event::HackerNewsResponse(response)).is_err() {
                eprintln!(
                    "[hacker-news] Request failed to deliver ({}); event channel closed.",
                    description
                );
                break;
            }
        }
    });
    match ready_receiver.recv() {
        Ok(Ok(())) => Ok(sender),
        Ok(Err(message)) => Err(message),
        Err(error) => Err(format!("Hacker News worker failed to start: {}", error)),
    }
}

pub fn relative_age(now: i64, timestamp: i64) -> String {
    let seconds = now.saturating_sub(timestamp);
    if seconds < 60 {
        return "just now".to_string();
    }
    if seconds < 60 * 60 {
        return format!("{}m ago", seconds / 60);
    }
    if seconds < 24 * 60 * 60 {
        return format!("{}h ago", seconds / (60 * 60));
    }
    if seconds < 30 * 24 * 60 * 60 {
        return format!("{}d ago", seconds / (24 * 60 * 60));
    }
    if seconds < 365 * 24 * 60 * 60 {
        return format!("{}mo ago", seconds / (30 * 24 * 60 * 60));
    }
    format!("{}y ago", seconds / (365 * 24 * 60 * 60))
}

fn host_of(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let host = rest
        .split(['/', '?', '#'])
        .next()?
        .rsplit('@')
        .next()?
        .split(':')
        .next()?;
    let host = host.strip_prefix("www.").unwrap_or(host);
    if host.is_empty()
        || host.len() > 60
        || !host.contains('.')
        || !host
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
    {
        return None;
    }
    Some(host.to_ascii_lowercase())
}

fn escape_text(value: &str) -> String {
    let value = decode_entities(value);
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if !matches!(character, '\t' | '\n' | '\r') && character < '\u{20}' {
            continue;
        }
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

fn escape_attribute(value: &str) -> String {
    escape_text(value)
}

fn safe_href(value: &str) -> Option<String> {
    let value = decode_entities(value);
    let value = value.trim();
    let lower = value.to_ascii_lowercase();
    if lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("mailto:")
        || lower.starts_with('#')
    {
        Some(escape_attribute(value))
    } else {
        None
    }
}

fn append_sanitized(node: crate::document::html::dom::NodeRef<'_>, output: &mut String) {
    match node.data() {
        NodeData::Text(data) | NodeData::Whitespace(data) => {
            output.push_str(&escape_text(&data.text));
        }
        NodeData::Root | NodeData::Wrapper(..) => {
            for child in node.children() {
                append_sanitized(child, output);
            }
        }
        NodeData::Element(element) => {
            let tag = element.name.to_ascii_lowercase();
            if matches!(
                tag.as_str(),
                "script" | "style" | "iframe" | "object" | "embed"
            ) {
                return;
            }
            if tag == "br" || tag == "hr" {
                output.push('<');
                output.push_str(&tag);
                output.push_str("/>");
                for child in node.children() {
                    append_sanitized(child, output);
                }
                return;
            }
            let allowed = matches!(
                tag.as_str(),
                "a" | "b"
                    | "blockquote"
                    | "code"
                    | "del"
                    | "div"
                    | "em"
                    | "h1"
                    | "h2"
                    | "h3"
                    | "h4"
                    | "h5"
                    | "h6"
                    | "i"
                    | "ins"
                    | "kbd"
                    | "li"
                    | "ol"
                    | "p"
                    | "pre"
                    | "s"
                    | "samp"
                    | "section"
                    | "small"
                    | "span"
                    | "strong"
                    | "sub"
                    | "sup"
                    | "u"
                    | "ul"
            );
            if !allowed {
                for child in node.children() {
                    append_sanitized(child, output);
                }
                return;
            }
            output.push('<');
            output.push_str(&tag);
            if tag == "a" {
                if let Some(href) = node.attribute("href").and_then(safe_href) {
                    output.push_str(" href=\"");
                    output.push_str(&href);
                    output.push('"');
                }
            }
            output.push('>');
            for child in node.children() {
                append_sanitized(child, output);
            }
            output.push_str("</");
            output.push_str(&tag);
            output.push('>');
        }
    }
}

fn sanitize_fragment(value: &str) -> String {
    let tree = XmlParser::new(value).parse();
    let mut output = String::new();
    for child in tree.root().children() {
        append_sanitized(child, &mut output);
    }
    if output.trim().is_empty() && !value.trim().is_empty() {
        escape_text(value)
    } else {
        output
    }
}

fn count_comments(children: &[HnItem]) -> usize {
    children
        .iter()
        .map(|child| 1 + count_comments(&child.children))
        .sum()
}

fn comment_key(path: &[usize]) -> String {
    let path = path
        .iter()
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join(".");
    format!("{}{}", COMMENT_COLLAPSE_PREFIX, path)
}

fn parse_comment_key(key: &str) -> Option<Vec<usize>> {
    let path = key.strip_prefix(COMMENT_COLLAPSE_PREFIX)?;
    if path.is_empty() {
        return None;
    }
    path.split('.')
        .map(|part| (!part.is_empty()).then(|| part.parse().ok()).flatten())
        .collect()
}

fn comment_at_path<'a>(children: &'a [HnItem], path: &[usize]) -> Option<&'a HnItem> {
    let (&index, rest) = path.split_first()?;
    let comment = children.get(index)?;
    if rest.is_empty() {
        Some(comment)
    } else {
        comment_at_path(&comment.children, rest)
    }
}

fn append_comment(
    item: &HnItem,
    depth: usize,
    now: i64,
    path: &[usize],
    collapsed_comments: &HashSet<String>,
    output: &mut String,
) {
    let body = sanitize_fragment(&item.text);
    let has_children = !item.children.is_empty();
    let deleted =
        item.deleted || item.dead || (item.author.trim().is_empty() && body.trim().is_empty());
    if !body.trim().is_empty() || has_children {
        let indent = depth.min(MAX_COMMENT_INDENT);
        output.push_str(&format!(
            "<div class=\"hn-comment\" style=\"margin-left: {}em\">",
            indent
        ));
        let author = if item.author.trim().is_empty() {
            "[deleted]"
        } else {
            item.author.as_str()
        };
        let key = comment_key(path);
        let collapsed = collapsed_comments.contains(&key);
        let marker = if has_children {
            if collapsed { "[+] " } else { "[-] " }
        } else {
            ""
        };
        let header = format!(
            "{}{} · {}{}",
            marker,
            author,
            relative_age(now, item.created_at_i),
            if has_children {
                let replies = item.children.len();
                format!(
                    " · {} {}",
                    replies,
                    if replies == 1 { "reply" } else { "replies" }
                )
            } else {
                String::new()
            }
        );
        output.push_str("<p>");
        // Reader has no script engine; the private link becomes a fold event.
        if has_children {
            output.push_str("<a href=\"");
            output.push_str(&escape_attribute(&key));
            output.push_str("\"><em>");
        } else {
            output.push_str("<em>");
        }
        output.push_str(&escape_text(&header));
        if has_children {
            output.push_str("</em></a>");
        } else {
            output.push_str("</em>");
        }
        output.push_str("</p>");
        if body.trim().is_empty() {
            output.push_str("<p>");
            output.push_str(if deleted {
                "[deleted]"
            } else {
                "[empty comment]"
            });
            output.push_str("</p>");
        } else {
            output.push_str(&body);
        }
        output.push_str("</div>");

        if collapsed {
            return;
        }
    }
    for (index, child) in item.children.iter().enumerate() {
        let mut child_path = path.to_vec();
        child_path.push(index);
        append_comment(
            child,
            depth.saturating_add(1),
            now,
            &child_path,
            collapsed_comments,
            output,
        );
    }
}

fn item_as_html_with_collapsed(item: &HnItem, collapsed_comments: &HashSet<String>) -> String {
    let title = if item.title.trim().is_empty() {
        "Hacker News story"
    } else {
        item.title.trim()
    };
    let title = escape_text(title);
    let author = if item.author.trim().is_empty() {
        "[deleted]"
    } else {
        item.author.as_str()
    };
    let comments = item
        .num_comments
        .max(u64::try_from(count_comments(&item.children)).unwrap_or(u64::MAX));
    let now = Utc::now().timestamp();
    let facts = format!(
        "by {} · {} · {} · {}",
        escape_text(author),
        escape_text(&count_label(item.points, "point")),
        escape_text(&count_label(comments, "comment")),
        escape_text(&relative_age(now, item.created_at_i))
    );

    let mut html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"/><title>{}</title></head><body><h1>{}</h1><p><em>{}</em></p>",
        title, title, facts
    );
    let story_text = if item.text.trim().is_empty() {
        item.story_text.as_str()
    } else {
        item.text.as_str()
    };
    let story_text = sanitize_fragment(story_text);
    if !story_text.trim().is_empty() {
        html.push_str("<div class=\"hn-self-post\">");
        html.push_str(&story_text);
        html.push_str("</div>");
    }
    if let Some(url) = safe_href(&item.url) {
        html.push_str("<p><a href=\"");
        html.push_str(&url);
        html.push_str("\">Open original article</a></p>");
    }
    html.push_str("<section class=\"hn-comments\">");
    for (index, child) in item.children.iter().enumerate() {
        append_comment(
            child,
            0,
            now,
            &[index],
            collapsed_comments,
            &mut html,
        );
    }
    html.push_str("</section></body></html>");
    html
}

pub fn item_as_html(item: &HnItem) -> String {
    item_as_html_with_collapsed(item, &HashSet::new())
}

fn capped_page_count(total_hits: usize, reported_pages: usize, hits_per_page: usize) -> usize {
    let hits_per_page = hits_per_page.max(1);
    let maximum = total_hits
        .min(MAX_STORIES)
        .saturating_add(hits_per_page - 1)
        / hits_per_page;
    reported_pages.max(1).min(maximum.max(1))
}

fn count_label(count: u64, noun: &str) -> String {
    if count == 1 {
        format!("1 {}", noun)
    } else {
        format!("{} {}s", count, noun)
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
struct ListRequest {
    request_id: u64,
    window: TimeWindow,
    page: usize,
    cutoff: i64,
}

#[derive(Debug, Clone, Eq, PartialEq)]
struct ItemRequest {
    request_id: u64,
    story_id: String,
}

pub struct Hn {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    worker: Option<Sender<ApiCommand>>,
    selected_window: TimeWindow,
    stories: Vec<HnHit>,
    current_page: usize,
    pages_count: usize,
    total_hits: usize,
    hits_per_page: usize,
    next_request_id: u64,
    list_request: Option<ListRequest>,
    item_request: Option<ItemRequest>,
    active_story_id: Option<String>,
    thread_item: Option<HnItem>,
    collapsed_comments: HashSet<String>,
    loading: bool,
    error: Option<String>,
    configured: bool,
}

impl Hn {
    pub fn new(rect: Rectangle, hub: &Hub, rq: &mut RenderQueue, context: &mut Context) -> Hn {
        let mut app = Hn {
            id: ID_FEEDER.next(),
            rect,
            children: Vec::new(),
            worker: None,
            selected_window: TimeWindow::Day,
            stories: Vec::new(),
            current_page: 0,
            pages_count: 1,
            total_hits: 0,
            hits_per_page: Self::hits_per_page(rect),
            next_request_id: 1,
            list_request: None,
            item_request: None,
            active_story_id: None,
            thread_item: None,
            collapsed_comments: HashSet::new(),
            loading: false,
            error: None,
            configured: false,
        };
        app.rebuild(rq, context);
        match spawn_api_worker(ALGOLIA_HN_API.to_string(), hub) {
            Ok(worker) => {
                app.worker = Some(worker);
                app.configured = true;
                app.request_list(true);
                app.rebuild(rq, context);
                if !context.online {
                    hub.send(Event::Notify(
                        "Waiting for a network connection.".to_string(),
                    ))
                    .ok();
                    if !context.settings.wifi {
                        hub.send(Event::SetWifi(true)).ok();
                    }
                }
            }
            Err(message) => {
                eprintln!(
                    "[hacker-news] API worker initialization failed: {}.",
                    message
                );
                app.error = Some(message.clone());
                hub.send(Event::Notify(format!(
                    "Can't initialize Hacker News: {}.",
                    message
                )))
                .ok();
                app.rebuild(rq, context);
            }
        }
        app
    }

    pub fn selected_window(&self) -> TimeWindow {
        self.selected_window
    }

    pub fn current_page(&self) -> usize {
        self.current_page
    }

    pub fn pages_count(&self) -> usize {
        self.pages_count
    }

    fn hits_per_page(rect: Rectangle) -> usize {
        let dpi = CURRENT_DEVICE.dpi;
        let bar_height = scale_by_dpi(SMALL_BAR_HEIGHT, dpi) as i32;
        let row_height = scale_by_dpi(BIG_BAR_HEIGHT, dpi) as i32;
        ((rect.height() as i32 - 2 * bar_height) / row_height).max(1) as usize
    }

    fn request_id(&mut self) -> u64 {
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
        request_id
    }

    fn send(&mut self, command: ApiCommand) -> bool {
        let description = command.description();
        if let Some(worker) = self.worker.as_ref() {
            if worker.send(command).is_ok() {
                return true;
            }
            eprintln!(
                "[hacker-news] Can't queue API command; worker stopped: {}.",
                description
            );
            self.configured = false;
        } else {
            eprintln!(
                "[hacker-news] Can't queue API command; worker is unavailable: {}.",
                description
            );
        }
        false
    }

    fn request_list(&mut self, clear: bool) {
        if clear {
            self.stories.clear();
            self.total_hits = 0;
            self.pages_count = 1;
            self.item_request = None;
            self.active_story_id = None;
            self.thread_item = None;
            self.collapsed_comments.clear();
        }
        if !self.configured {
            self.loading = false;
            return;
        }
        let cutoff = self.selected_window.cutoff(Utc::now().timestamp());
        let request = ListRequest {
            request_id: self.request_id(),
            window: self.selected_window,
            page: self.current_page,
            cutoff,
        };
        self.list_request = Some(request);
        self.loading = true;
        self.error = None;
        if !self.send(ApiCommand::ListStories {
            request_id: request.request_id,
            window: request.window,
            page: request.page,
            hits_per_page: self.hits_per_page,
            cutoff: request.cutoff,
        }) {
            self.list_request = None;
            self.loading = false;
            self.error = Some("Hacker News worker is unavailable.".to_string());
        }
    }

    fn request_item(&mut self, story_id: String) {
        if validate_story_id(&story_id).is_err() {
            self.active_story_id = None;
            self.item_request = None;
            self.error = Some("The story has an invalid ID.".to_string());
            return;
        }
        let request = ItemRequest {
            request_id: self.request_id(),
            story_id: story_id.clone(),
        };
        self.active_story_id = Some(story_id.clone());
        self.item_request = Some(request.clone());
        self.error = None;
        if !self.send(ApiCommand::GetItem {
            request_id: request.request_id,
            story_id,
        }) {
            self.item_request = None;
            self.active_story_id = None;
            self.error = Some("Hacker News worker is unavailable.".to_string());
        }
    }

    fn toggle_comment(&mut self, key: &str, hub: &Hub) {
        let Some(path) = parse_comment_key(key) else {
            return;
        };
        let has_children = self
            .thread_item
            .as_ref()
            .and_then(|item| comment_at_path(&item.children, &path))
            .is_some_and(|comment| !comment.children.is_empty());
        if !has_children {
            return;
        }
        if !self.collapsed_comments.remove(key) {
            self.collapsed_comments.insert(key.to_string());
        }
        if let Some(item) = self.thread_item.as_ref() {
            hub.send(Event::HackerNewsThreadUpdated {
                html: item_as_html_with_collapsed(item, &self.collapsed_comments),
                link_uri: key.to_string(),
            })
            .ok();
        }
    }

    fn select_window(&mut self, window: TimeWindow, rq: &mut RenderQueue, context: &mut Context) {
        if self.selected_window == window {
            if !self.loading && (self.error.is_some() || self.stories.is_empty()) {
                self.request_list(false);
                self.rebuild(rq, context);
            }
            return;
        }
        self.selected_window = window;
        self.current_page = 0;
        self.list_request = None;
        self.item_request = None;
        self.request_list(true);
        self.rebuild(rq, context);
    }

    fn close_title_menu(&mut self, rq: &mut RenderQueue) {
        if let Some(index) = self
            .children
            .iter()
            .position(|child| child.view_id() == Some(ViewId::TitleMenu))
        {
            rq.add(RenderData::expose(
                *self.child(index).rect(),
                UpdateMode::Gui,
            ));
            self.children.remove(index);
        }
    }

    fn toggle_title_menu(&mut self, rect: Rectangle, rq: &mut RenderQueue, context: &mut Context) {
        if self
            .children
            .iter()
            .any(|child| child.view_id() == Some(ViewId::TitleMenu))
        {
            self.close_title_menu(rq);
            return;
        }
        let entries = vec![EntryKind::Command(
            "Refresh".to_string(),
            EntryId::HackerNewsRefresh,
        )];
        let menu = Menu::new(
            rect,
            ViewId::TitleMenu,
            MenuKind::DropDown,
            entries,
            context,
        );
        rq.add(RenderData::new(menu.id(), *menu.rect(), UpdateMode::Gui));
        self.children.push(Box::new(menu));
    }

    fn handle_response(
        &mut self,
        response: &HnResponse,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) {
        match response {
            HnResponse::Stories {
                request_id,
                window,
                page,
                cutoff,
                result,
            } => {
                let Some(request) = self.list_request else {
                    return;
                };
                if request.request_id != *request_id
                    || request.window != *window
                    || request.page != *page
                    || request.cutoff != *cutoff
                    || self.selected_window != *window
                {
                    eprintln!("[hacker-news] Dropping stale list response {}.", request_id);
                    return;
                }
                self.list_request = None;
                self.loading = false;
                self.total_hits = result.nb_hits.min(MAX_STORIES);
                self.pages_count = capped_page_count(
                    self.total_hits,
                    result.nb_pages,
                    self.hits_per_page,
                );
                if self.current_page >= self.pages_count {
                    self.current_page = self.pages_count - 1;
                    self.request_list(true);
                    self.rebuild(rq, context);
                    return;
                }
                let page_start = request.page.saturating_mul(self.hits_per_page);
                let page_limit = if self.total_hits == 0 {
                    result.hits.len().min(MAX_STORIES).min(self.hits_per_page)
                } else {
                    self.total_hits
                        .saturating_sub(page_start)
                        .min(self.hits_per_page)
                };
                self.stories = result
                    .hits
                    .iter()
                    .filter(|story| story.created_at_i == 0 || story.created_at_i >= request.cutoff)
                    .filter_map(|story| {
                        let id = validate_story_id(&story.object_id).ok()?;
                        if story.title.trim().is_empty() {
                            return None;
                        }
                        let mut story = story.clone();
                        story.object_id = id.to_string();
                        Some(story)
                    })
                    .take(page_limit)
                    .collect();
                self.error = None;
                self.rebuild(rq, context);
            }
            HnResponse::Item {
                request_id,
                story_id,
                item,
            } => {
                let Some(request) = self.item_request.as_ref() else {
                    return;
                };
                if request.request_id != *request_id || request.story_id != *story_id {
                    eprintln!(
                        "[hacker-news] Dropping stale story response {}.",
                        request_id
                    );
                    return;
                }
                let item_id = if item.id.is_empty() {
                    None
                } else {
                    validate_story_id(&item.id).ok().map(|id| id.to_string())
                };
                if !item.id.is_empty() && item_id.as_deref() != Some(story_id.as_str()) {
                    self.item_request = None;
                    self.active_story_id = None;
                    self.error = Some("Hacker News returned the wrong story.".to_string());
                    hub.send(Event::Notify(
                        "Hacker News returned the wrong story.".to_string(),
                    ))
                    .ok();
                    self.rebuild(rq, context);
                    return;
                }
                self.item_request = None;
                self.active_story_id = None;
                let mut item = item.clone();
                if item.story_text.is_empty() {
                    item.story_text = self
                        .stories
                        .iter()
                        .find(|story| story.object_id == *story_id)
                        .map(|story| story.story_text.clone())
                        .unwrap_or_default();
                }
                self.thread_item = Some(item.clone());
                self.collapsed_comments.clear();
                self.error = None;
                self.rebuild(rq, context);
                hub.send(Event::OpenHtml(item_as_html(&item), None)).ok();
            }
            HnResponse::Error {
                request_id,
                window,
                page,
                story_id,
                message,
            } => {
                let list_matches = self.list_request.is_some_and(|request| {
                    request.request_id == *request_id
                        && window == &Some(request.window)
                        && page == &Some(request.page)
                });
                let item_matches = self.item_request.as_ref().is_some_and(|request| {
                    request.request_id == *request_id
                        && story_id.as_deref() == Some(request.story_id.as_str())
                });
                if !list_matches && !item_matches {
                    eprintln!(
                        "[hacker-news] Dropping stale error response {}.",
                        request_id
                    );
                    return;
                }
                if list_matches {
                    self.list_request = None;
                    self.loading = false;
                }
                if item_matches {
                    self.item_request = None;
                    self.active_story_id = None;
                }
                self.error = Some(message.clone());
                eprintln!(
                    "[hacker-news] API response {} reported an error: {}.",
                    request_id, message
                );
                hub.send(Event::Notify(format!("Hacker News: {}.", message)))
                    .ok();
                self.rebuild(rq, context);
            }
        }
    }

    fn rebuild(&mut self, rq: &mut RenderQueue, context: &mut Context) {
        self.children.clear();
        let dpi = CURRENT_DEVICE.dpi;
        let side = scale_by_dpi(SMALL_BAR_HEIGHT, dpi) as i32;
        let thickness = scale_by_dpi(THICKNESS_MEDIUM, dpi) as i32;
        let (small_thickness, big_thickness) = halves(thickness);
        let top_bar = TopBar::new(
            rect![
                self.rect.min.x,
                self.rect.min.y,
                self.rect.max.x,
                self.rect.min.y + side - small_thickness
            ],
            Event::Back,
            format!("Hacker News — {}", self.selected_window.label()),
            context,
        );
        self.children.push(Box::new(top_bar));
        self.children.push(Box::new(Filler::new(
            rect![
                self.rect.min.x,
                self.rect.min.y + side - small_thickness,
                self.rect.max.x,
                self.rect.min.y + side + big_thickness
            ],
            BLACK,
        )));

        let content_min_y = self.rect.min.y + side + big_thickness;
        let content_max_y = (self.rect.max.y - side - small_thickness).max(content_min_y);
        let content_rect = rect![
            self.rect.min.x,
            content_min_y,
            self.rect.max.x,
            content_max_y
        ];
        if self.stories.is_empty() {
            let text = if self.loading {
                "Loading Hacker News…".to_string()
            } else if let Some(error) = self.error.as_ref() {
                format!("Hacker News: {}", error)
            } else {
                format!(
                    "No stories in the last {}.",
                    self.selected_window.label().to_ascii_lowercase()
                )
            };
            self.children
                .push(Box::new(Label::new(content_rect, text, Align::Center)));
        } else {
            let heights =
                crate::geom::divide(content_rect.height() as i32, self.hits_per_page as i32);
            let thickness = scale_by_dpi(THICKNESS_MEDIUM, dpi) as i32;
            let (small_thickness, big_thickness) = halves(thickness);
            let mut y = content_rect.min.y;
            for (index, story) in self.stories.iter().cloned().enumerate() {
                let height = heights.get(index).copied().unwrap_or_default();
                let has_next = index + 1 < self.stories.len();
                let row_min = y + if index > 0 { big_thickness } else { 0 };
                let row_max = y + height - if has_next { small_thickness } else { 0 };
                let active = self.active_story_id.as_deref() == Some(story.object_id.as_str());
                let loading = active && self.item_request.is_some();
                self.children.push(Box::new(HnStoryRow::new(
                    rect![content_rect.min.x, row_min, content_rect.max.x, row_max],
                    story,
                    active,
                    loading,
                    self.loading,
                )));
                if has_next {
                    self.children.push(Box::new(Filler::new(
                        rect![
                            content_rect.min.x,
                            row_max,
                            content_rect.max.x,
                            row_max + thickness
                        ],
                        SEPARATOR_NORMAL,
                    )));
                }
                y += height;
            }
            if y < content_rect.max.y {
                self.children.push(Box::new(Filler::new(
                    rect![
                        content_rect.min.x,
                        y,
                        content_rect.max.x,
                        content_rect.max.y
                    ],
                    WHITE,
                )));
            }
        }

        self.children.push(Box::new(Filler::new(
            rect![
                self.rect.min.x,
                self.rect.max.y - side - small_thickness,
                self.rect.max.x,
                self.rect.max.y - side
            ],
            BLACK,
        )));
        let bottom = HnBottomBar::new(
            rect![
                self.rect.min.x,
                self.rect.max.y - side,
                self.rect.max.x,
                self.rect.max.y
            ],
            self.selected_window,
            self.current_page,
            self.pages_count,
        );
        self.children.push(Box::new(bottom));
        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Full));
    }
}

impl View for Hn {
    fn handle_event(
        &mut self,
        evt: &Event,
        hub: &Hub,
        _bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        match evt {
            Event::HackerNewsResponse(response) => {
                self.handle_response(response, hub, rq, context);
                true
            }
            Event::HackerNewsOpen(story_id) => {
                if self
                    .stories
                    .iter()
                    .any(|story| story.object_id == *story_id)
                {
                    self.request_item(story_id.clone());
                    self.rebuild(rq, context);
                }
                true
            }
            Event::HackerNewsToggleComment(key) => {
                self.toggle_comment(key, hub);
                true
            }
            Event::Select(EntryId::HackerNewsWindow(window)) => {
                self.select_window(*window, rq, context);
                self.close_title_menu(rq);
                true
            }
            Event::Select(EntryId::HackerNewsRefresh) => {
                self.request_list(false);
                self.close_title_menu(rq);
                self.rebuild(rq, context);
                true
            }
            Event::Page(direction) => {
                let next = match direction {
                    CycleDir::Previous => self.current_page.saturating_sub(1),
                    CycleDir::Next => self
                        .current_page
                        .saturating_add(1)
                        .min(self.pages_count.saturating_sub(1)),
                };
                if next != self.current_page {
                    self.current_page = next;
                    self.request_list(true);
                    self.rebuild(rq, context);
                }
                true
            }
            Event::Device(DeviceEvent::NetUp) => {
                self.request_list(false);
                self.rebuild(rq, context);
                true
            }
            Event::Reseed => {
                self.rebuild(rq, context);
                true
            }
            Event::ToggleNear(ViewId::TitleMenu, rect) => {
                self.toggle_title_menu(*rect, rq, context);
                true
            }
            Event::ToggleNear(ViewId::MainMenu, rect) => {
                toggle_main_menu(self, *rect, None, rq, context);
                true
            }
            Event::ToggleNear(ViewId::BatteryMenu, rect) => {
                toggle_battery_menu(self, *rect, None, rq, context);
                true
            }
            Event::ToggleNear(ViewId::ClockMenu, rect) => {
                toggle_clock_menu(self, *rect, None, rq, context);
                true
            }
            Event::Back | Event::Select(EntryId::Quit) | Event::Select(EntryId::Reboot) => false,
            _ => false,
        }
    }

    fn render(&self, fb: &mut dyn Framebuffer, _rect: Rectangle, _fonts: &mut Fonts) {
        fb.draw_rectangle(&self.rect, WHITE);
    }

    fn resize(&mut self, rect: Rectangle, _hub: &Hub, rq: &mut RenderQueue, context: &mut Context) {
        self.rect = rect;
        self.hits_per_page = Self::hits_per_page(rect);
        self.current_page = 0;
        self.list_request = None;
        self.request_list(true);
        self.rebuild(rq, context);
    }

    fn rect(&self) -> &Rectangle {
        &self.rect
    }

    fn rect_mut(&mut self) -> &mut Rectangle {
        &mut self.rect
    }

    fn children(&self) -> &Vec<Box<dyn View>> {
        &self.children
    }

    fn children_mut(&mut self) -> &mut Vec<Box<dyn View>> {
        &mut self.children
    }

    fn id(&self) -> Id {
        self.id
    }

    fn view_id(&self) -> Option<ViewId> {
        Some(ViewId::HackerNews)
    }
}

struct HnStoryRow {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    story: HnHit,
    active: bool,
    loading: bool,
    refreshing: bool,
}

impl HnStoryRow {
    fn new(
        rect: Rectangle,
        story: HnHit,
        active: bool,
        loading: bool,
        refreshing: bool,
    ) -> HnStoryRow {
        HnStoryRow {
            id: ID_FEEDER.next(),
            rect,
            children: Vec::new(),
            story,
            active,
            loading,
            refreshing,
        }
    }
}

impl View for HnStoryRow {
    fn handle_event(
        &mut self,
        evt: &Event,
        hub: &Hub,
        _bus: &mut Bus,
        rq: &mut RenderQueue,
        _context: &mut Context,
    ) -> bool {
        match evt {
            Event::Gesture(GestureEvent::Tap(center)) if self.rect.includes(*center) => {
                self.active = true;
                self.loading = true;
                rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
                hub.send(Event::HackerNewsOpen(self.story.object_id.clone()))
                    .ok();
                true
            }
            _ => false,
        }
    }

    fn render(&self, fb: &mut dyn Framebuffer, _rect: Rectangle, fonts: &mut Fonts) {
        let scheme = if self.active {
            TEXT_INVERTED_HARD
        } else {
            TEXT_NORMAL
        };
        fb.draw_rectangle(&self.rect, scheme[0]);
        let dpi = CURRENT_DEVICE.dpi;
        let padding = font_from_style(fonts, &NORMAL_STYLE, dpi).em() as i32;
        let title_font = font_from_style(fonts, &MD_TITLE, dpi);
        let title_width = self.rect.width() as i32 - 2 * padding;
        let title = title_font.plan(&self.story.title, Some(title_width.max(1)), None);
        let title_y = self.rect.min.y + padding + title_font.ascender();
        title_font.render(
            fb,
            scheme[1],
            &title,
            pt!(self.rect.min.x + padding, title_y),
        );

        let source = if self.story.url.trim().is_empty() {
            if self.story.author.trim().is_empty() {
                "[deleted]".to_string()
            } else {
                self.story.author.clone()
            }
        } else {
            host_of(&self.story.url).unwrap_or_else(|| self.story.author.clone())
        };
        let metadata = if self.loading {
            "Loading thread…".to_string()
        } else if self.refreshing {
            "Refreshing Hacker News…".to_string()
        } else {
            format!(
                "{} · {} · {} · {}",
                source,
                count_label(self.story.points, "point"),
                count_label(self.story.num_comments, "comment"),
                relative_age(Utc::now().timestamp(), self.story.created_at_i)
            )
        };
        let detail_font = font_from_style(fonts, &MD_AUTHOR, dpi);
        let detail_width = self.rect.width() as i32 - 2 * padding;
        let detail = detail_font.plan(&metadata, Some(detail_width.max(1)), None);
        let detail_y = self.rect.max.y - padding / 2;
        detail_font.render(
            fb,
            scheme[1],
            &detail,
            pt!(self.rect.min.x + padding, detail_y),
        );
    }

    fn rect(&self) -> &Rectangle {
        &self.rect
    }

    fn rect_mut(&mut self) -> &mut Rectangle {
        &mut self.rect
    }

    fn children(&self) -> &Vec<Box<dyn View>> {
        &self.children
    }

    fn children_mut(&mut self) -> &mut Vec<Box<dyn View>> {
        &mut self.children
    }

    fn id(&self) -> Id {
        self.id
    }
}

struct HnTab {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    window: TimeWindow,
    selected: bool,
    active: bool,
}

impl HnTab {
    fn new(rect: Rectangle, window: TimeWindow, selected: bool) -> HnTab {
        HnTab {
            id: ID_FEEDER.next(),
            rect,
            children: Vec::new(),
            window,
            selected,
            active: false,
        }
    }
}

impl View for HnTab {
    fn handle_event(
        &mut self,
        evt: &Event,
        _hub: &Hub,
        bus: &mut Bus,
        rq: &mut RenderQueue,
        _context: &mut Context,
    ) -> bool {
        match evt {
            Event::Device(DeviceEvent::Finger {
                status, position, ..
            }) => match status {
                crate::input::FingerStatus::Down if self.rect.includes(*position) => {
                    self.active = true;
                    rq.add(RenderData::new(self.id, self.rect, UpdateMode::Fast));
                    true
                }
                crate::input::FingerStatus::Up if self.active => {
                    self.active = false;
                    rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
                    true
                }
                _ => false,
            },
            Event::Gesture(GestureEvent::Tap(center)) if self.rect.includes(*center) => {
                bus.push_back(Event::Select(EntryId::HackerNewsWindow(self.window)));
                true
            }
            _ => false,
        }
    }

    fn render(&self, fb: &mut dyn Framebuffer, _rect: Rectangle, fonts: &mut Fonts) {
        let scheme = if self.selected || self.active {
            TEXT_INVERTED_HARD
        } else {
            TEXT_NORMAL
        };
        fb.draw_rectangle(&self.rect, scheme[0]);
        let dpi = CURRENT_DEVICE.dpi;
        let font = font_from_style(fonts, &NORMAL_STYLE, dpi);
        let plan = font.plan(self.window.label(), Some(self.rect.width() as i32), None);
        let dx = (self.rect.width() as i32 - plan.width) / 2;
        let dy = (self.rect.height() as i32 - font.x_heights.0 as i32) / 2;
        font.render(
            fb,
            scheme[1],
            &plan,
            pt!(self.rect.min.x + dx, self.rect.max.y - dy),
        );
    }

    fn rect(&self) -> &Rectangle {
        &self.rect
    }

    fn rect_mut(&mut self) -> &mut Rectangle {
        &mut self.rect
    }

    fn children(&self) -> &Vec<Box<dyn View>> {
        &self.children
    }

    fn children_mut(&mut self) -> &mut Vec<Box<dyn View>> {
        &mut self.children
    }

    fn id(&self) -> Id {
        self.id
    }
}

struct HnBottomBar {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
}

impl HnBottomBar {
    fn new(
        rect: Rectangle,
        selected: TimeWindow,
        current_page: usize,
        pages_count: usize,
    ) -> HnBottomBar {
        let side = rect.height() as i32;
        let middle_width = (rect.width() as i32 - 2 * side).max(0);
        let widths = crate::geom::divide(middle_width, 3);
        let prev_rect = rect![rect.min, rect.min + side];
        let next_rect = rect![rect.max - side, rect.max];
        let prev: Box<dyn View> = if current_page > 0 {
            Box::new(Icon::new(
                "arrow-left",
                prev_rect,
                Event::Page(CycleDir::Previous),
            ))
        } else {
            Box::new(Filler::new(prev_rect, WHITE))
        };
        let next: Box<dyn View> = if current_page.saturating_add(1) < pages_count.max(1) {
            Box::new(Icon::new(
                "arrow-right",
                next_rect,
                Event::Page(CycleDir::Next),
            ))
        } else {
            Box::new(Filler::new(next_rect, WHITE))
        };
        let mut children: Vec<Box<dyn View>> = Vec::with_capacity(5);
        children.push(prev);
        let mut x = rect.min.x + side;
        for (index, window) in TimeWindow::ALL.into_iter().enumerate() {
            let width = widths.get(index).copied().unwrap_or_default();
            children.push(Box::new(HnTab::new(
                rect![x, rect.min.y, x + width, rect.max.y],
                window,
                selected == window,
            )));
            x += width;
        }
        children.push(next);
        HnBottomBar {
            id: ID_FEEDER.next(),
            rect,
            children,
        }
    }
}

impl View for HnBottomBar {
    fn handle_event(
        &mut self,
        evt: &Event,
        _hub: &Hub,
        bus: &mut Bus,
        _rq: &mut RenderQueue,
        _context: &mut Context,
    ) -> bool {
        match evt {
            Event::Gesture(GestureEvent::Swipe {
                dir: Dir::West,
                start,
                ..
            }) if self.rect.includes(*start) => {
                bus.push_back(Event::Page(CycleDir::Next));
                true
            }
            Event::Gesture(GestureEvent::Swipe {
                dir: Dir::East,
                start,
                ..
            }) if self.rect.includes(*start) => {
                bus.push_back(Event::Page(CycleDir::Previous));
                true
            }
            _ => false,
        }
    }

    fn render(&self, _fb: &mut dyn Framebuffer, _rect: Rectangle, _fonts: &mut Fonts) {}

    fn resize(&mut self, rect: Rectangle, hub: &Hub, rq: &mut RenderQueue, context: &mut Context) {
        let side = rect.height() as i32;
        let middle_width = (rect.width() as i32 - 2 * side).max(0);
        let widths = crate::geom::divide(middle_width, 3);
        self.children[0].resize(rect![rect.min, rect.min + side], hub, rq, context);
        let mut x = rect.min.x + side;
        for index in 0..3 {
            let width = widths.get(index).copied().unwrap_or_default();
            self.children[index + 1].resize(
                rect![x, rect.min.y, x + width, rect.max.y],
                hub,
                rq,
                context,
            );
            x += width;
        }
        self.children[4].resize(rect![rect.max - side, rect.max], hub, rq, context);
        self.rect = rect;
    }

    fn rect(&self) -> &Rectangle {
        &self.rect
    }

    fn rect_mut(&mut self) -> &mut Rectangle {
        &mut self.rect
    }

    fn children(&self) -> &Vec<Box<dyn View>> {
        &self.children
    }

    fn children_mut(&mut self) -> &mut Vec<Box<dyn View>> {
        &mut self.children
    }

    fn id(&self) -> Id {
        self.id
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Document;
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn server(body: &str) -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let body = body.to_string();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 4096];
            loop {
                let count = stream.read(&mut buffer).unwrap();
                request.extend_from_slice(&buffer[..count]);
                if request.windows(4).any(|part| part == b"\r\n\r\n") || count == 0 {
                    break;
                }
            }
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
            String::from_utf8(request).unwrap()
        });
        (format!("http://{}", address), handle)
    }

    #[test]
    fn windows_have_exact_rolling_cutoffs() {
        let now = 1_700_000_000;
        assert_eq!(TimeWindow::Day.cutoff(now), now - 86_400);
        assert_eq!(TimeWindow::Week.cutoff(now), now - 7 * 86_400);
        assert_eq!(TimeWindow::Month.cutoff(now), now - 30 * 86_400);
        assert_eq!(TimeWindow::ALL.map(TimeWindow::index), [0, 1, 2]);
    }

    #[test]
    fn search_uses_ranked_story_query_and_requested_page() {
        let (domain, request) =
            server(r#"{"hits":[],"nbHits":0,"page":2,"nbPages":0,"hitsPerPage":7}"#);
        let api = Api::new(domain).unwrap();
        let result = api.search(2, 7, 1_699_913_600).unwrap();
        assert!(result.hits.is_empty());
        let request = request.join().unwrap();
        assert!(request.starts_with("GET /api/v1/search?"));
        assert!(request.contains("tags=story"));
        assert!(request.contains("numericFilters=created_at_i%3E%3D1699913600"));
        assert!(request.contains("page=2"));
        assert!(request.contains("hitsPerPage=7"));
        assert!(!request.contains("search_by_date"));
    }

    #[test]
    fn comments_use_hcker_api_and_map_nested_html_comments() {
        let (domain, request) = server(
            r#"{"storyId":49489982,"storyTitle":"A","storyUrl":"https://example.com","storyAuthor":"author","storyScore":42,"storyTime":"2026-08-29T14:02:10Z","comments":[{"id":1,"parentId":49489982,"author":"one","createdAt":"2026-08-29T14:35:12Z","points":null,"rawHtml":"<p>one</p>","depth":0,"isDeleted":false,"children":[{"id":2,"parentId":1,"author":"two","createdAt":"2026-08-29T14:36:12Z","rawHtml":"two","isDeleted":true,"children":[]}] }],"totalCount":2,"maxDepth":1}"#,
        );
        let api = Api::new_with_comments_domain(domain.clone(), domain.clone()).unwrap();
        let item = api.comments("49489982").unwrap();
        assert_eq!(item.id, "49489982");
        assert_eq!(item.title, "A");
        assert_eq!(item.points, 42);
        assert_eq!(item.created_at_i, 1_788_012_130);
        assert_eq!(item.children.len(), 1);
        assert_eq!(item.children[0].created_at_i, 1_788_014_112);
        assert_eq!(item.children[0].text, "<p>one</p>");
        assert_eq!(item.children[0].children[0].text, "two");
        assert!(item.children[0].children[0].deleted);

        let request = request.join().unwrap().to_ascii_lowercase();
        assert!(request.starts_with("get /api/comments/49489982 "));
        assert!(request.contains("accept: application/json"));
        assert!(request.contains(&format!("referer: {}/", domain).to_ascii_lowercase()));
    }

    #[test]
    fn invalid_story_ids_are_rejected_before_a_request() {
        for id in ["", "0", "-1", "1/other", "1?x=y", "abc"] {
            assert!(validate_story_id(id).is_err(), "accepted {:?}", id);
        }
        assert_eq!(validate_story_id("001").unwrap(), 1);
    }

    #[test]
    fn defensive_models_keep_valid_nested_items() {
        let value: HnSearchResult = serde_json::from_str(
            r#"{"hits":[{"objectID":"12","title":"A","points":null,"num_comments":"2","created_at_i":10},{"objectID":false},{"objectID":"13","title":"B"}],"nbPages":2}"#,
        )
        .unwrap();
        assert_eq!(value.hits.len(), 3);
        assert_eq!(value.hits[0].points, 0);
        assert_eq!(value.hits[0].num_comments, 2);
        assert_eq!(value.nb_pages, 2);

        let item: HnItem = serde_json::from_str(
            r#"{"id":99,"title":"A","children":[{"id":100,"text":"x","children":[{"id":101,"text":"y"}]},null]}"#,
        )
        .unwrap();
        assert_eq!(item.id, "99");
        assert_eq!(item.children.len(), 1);
        assert_eq!(item.children[0].children[0].id, "101");
    }

    #[test]
    fn story_pages_are_capped_at_fifty_results() {
        assert_eq!(capped_page_count(0, 20, 7), 1);
        assert_eq!(capped_page_count(50, 20, 7), 8);
        assert_eq!(capped_page_count(500, 20, 7), 8);
        assert_eq!(capped_page_count(50, 3, 50), 1);
    }

    #[test]
    fn comment_fold_links_hide_only_the_selected_subtree() {
        let item = HnItem {
            children: vec![
                HnItem {
                    author: "top".to_string(),
                    text: "top body".to_string(),
                    children: vec![HnItem {
                        author: "reply".to_string(),
                        text: "reply body".to_string(),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                HnItem {
                    author: "sibling".to_string(),
                    text: "sibling body".to_string(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let key = comment_key(&[0]);
        let open = item_as_html(&item);
        assert!(open.contains("href=\"hn-collapse:0\""));
        assert!(open.contains("reply body"));

        let mut collapsed = HashSet::new();
        collapsed.insert(key);
        let folded = item_as_html_with_collapsed(&item, &collapsed);
        assert!(folded.contains("top body"));
        assert!(folded.contains("sibling body"));
        assert!(!folded.contains("reply body"));
        assert!(folded.contains("[+] top"));
    }

    #[test]
    fn html_escapes_metadata_sanitizes_content_and_clamps_depth() {
        let mut deep = HnItem {
            id: "1".to_string(),
            title: "A <story> & \"title\"".to_string(),
            url: "https://example.com/?a=1&b=2".to_string(),
            author: "<author>".to_string(),
            text: "<p>Self <b>post</b><br></p>".to_string(),
            ..Default::default()
        };
        let mut current = &mut deep;
        for id in 2..=10 {
            current.children.push(HnItem {
                id: id.to_string(),
                author: "comment".to_string(),
                text: "<p>hello</p>".to_string(),
                ..Default::default()
            });
            current = current.children.last_mut().unwrap();
        }
        let html = item_as_html(&deep);
        assert!(html.contains("<meta charset=\"utf-8\"/>"));
        assert!(html.contains("<title>A &lt;story&gt; &amp; &quot;title&quot;</title>"));
        assert!(html.contains("Open original article"));
        assert!(html.contains("margin-left: 6em"));
        assert!(!html.contains("vote"));
        assert!(!html.contains("reply to"));
        let document = crate::document::html::HtmlDocument::new_from_memory(&html);
        assert_eq!(document.title().as_deref(), Some("A <story> & \"title\""));
    }

    #[test]
    fn relative_ages_are_stable_and_future_items_are_now() {
        assert_eq!(relative_age(100, 100), "just now");
        assert_eq!(relative_age(100, 40), "1m ago");
        assert_eq!(relative_age(100, 0), "1m ago");
        assert_eq!(relative_age(0, 10), "just now");
    }
}
