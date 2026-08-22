use crate::color::{BLACK, TEXT_INVERTED_HARD, TEXT_NORMAL, WHITE};
use crate::context::Context;
use crate::device::CURRENT_DEVICE;
use crate::font::{font_from_style, Fonts, MD_AUTHOR, MD_TITLE, NORMAL_STYLE};
use crate::framebuffer::{Framebuffer, UpdateMode};
use crate::geom::{halves, CycleDir, Dir, Rectangle};
use crate::gesture::GestureEvent;
use crate::input::DeviceEvent;
use crate::unit::scale_by_dpi;
use crate::view::common::{locate_by_id, toggle_battery_menu, toggle_clock_menu, toggle_main_menu};
use crate::view::filler::Filler;
use crate::view::icon::Icon;
use crate::view::label::Label;
use crate::view::menu::{Menu, MenuKind};
use crate::view::page_label::PageLabel;
use crate::view::top_bar::TopBar;
use crate::view::Align;
use crate::view::{Bus, EntryId, EntryKind, Event, Hub, Id, ID_FEEDER};
use crate::view::{
    RenderData, RenderQueue, View, ViewId, BIG_BAR_HEIGHT, SMALL_BAR_HEIGHT, THICKNESS_MEDIUM,
};
use reqwest::blocking::{Client, Response};
use reqwest::header::CONTENT_TYPE;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;
use std::sync::mpsc::{self, Sender};
use std::sync::Once;
use std::thread;
use std::time::Duration;

const AUTH_HEADER: &str = "X-Auth-Token";
static TLS_PROVIDER: Once = Once::new();

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum MinifluxStatus {
    Read,
    Unread,
}

impl MinifluxStatus {
    fn as_str(self) -> &'static str {
        match self {
            MinifluxStatus::Read => "read",
            MinifluxStatus::Unread => "unread",
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MinifluxCategory {
    pub id: u64,
    pub title: String,
    pub total_unread: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MinifluxFeed {
    pub title: String,
    pub site_url: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MinifluxEntry {
    pub id: u64,
    pub title: String,
    pub url: String,
    pub author: String,
    pub content: String,
    pub published_at: String,
    pub feed: MinifluxFeed,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct EntriesResult {
    total: usize,
    entries: Vec<MinifluxEntry>,
}

enum ApiCommand {
    ListCategories {
        request_id: u64,
    },
    ListEntries {
        request_id: u64,
        category_id: Option<u64>,
        offset: usize,
        limit: usize,
    },
    GetEntry {
        request_id: u64,
        entry_id: u64,
    },
    SetStatus {
        request_id: u64,
        entry_id: u64,
        status: MinifluxStatus,
    },
}

impl ApiCommand {
    fn description(&self) -> String {
        match *self {
            ApiCommand::ListCategories { .. } => "list categories".to_string(),
            ApiCommand::ListEntries {
                category_id,
                offset,
                limit,
                ..
            } => format!(
                "list entries (category={:?}, offset={}, limit={})",
                category_id, offset, limit
            ),
            ApiCommand::GetEntry { entry_id, .. } => format!("get entry {}", entry_id),
            ApiCommand::SetStatus {
                entry_id, status, ..
            } => format!("set entry {} status to {}", entry_id, status.as_str()),
        }
    }
}

struct Api {
    client: Client,
    domain: String,
    api_key: String,
}

impl Api {
    fn new(domain: String, api_key: String) -> Result<Api, String> {
        let domain = domain.trim_end_matches('/').to_string();
        eprintln!("[miniflux] Initializing API client for {}.", domain);
        TLS_PROVIDER.call_once(|| {
            match rustls::crypto::ring::default_provider().install_default() {
                Ok(()) => eprintln!("[miniflux] Installed the rustls ring crypto provider."),
                Err(_) => eprintln!("[miniflux] A rustls crypto provider was already installed."),
            }
        });
        let client = Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|e| {
                let message = format!("can't build HTTP client: {}", e);
                eprintln!("[miniflux] {}.", message);
                message
            })?;
        eprintln!("[miniflux] API client initialized with a 20 second timeout.");
        Ok(Api {
            client,
            domain,
            api_key,
        })
    }

    fn checked_json(operation: &str, response: Response) -> Result<JsonValue, String> {
        let status = response.status();
        eprintln!("[miniflux] {} returned HTTP {}.", operation, status);
        if status.is_success() {
            response.json().map_err(|e| {
                let message = format!("invalid JSON response: {}", e);
                eprintln!("[miniflux] {} failed: {}.", operation, message);
                message
            })
        } else {
            let reason = status.canonical_reason().unwrap_or("HTTP error");
            let message = response
                .json::<JsonValue>()
                .ok()
                .and_then(|body| {
                    body.get("error_message")
                        .and_then(JsonValue::as_str)
                        .map(str::to_string)
                })
                .unwrap_or_else(|| format!("{} {}", status.as_u16(), reason));
            eprintln!("[miniflux] {} failed: {}.", operation, message);
            Err(message)
        }
    }

    fn transport_error(operation: &str, error: reqwest::Error) -> String {
        let message = format!("{} transport error: {}", operation, error);
        eprintln!("[miniflux] {}.", message);
        message
    }

    fn categories(&self) -> Result<JsonValue, String> {
        let operation = "GET /v1/categories?counts=true";
        let response = self
            .client
            .get(format!("{}/v1/categories", self.domain))
            .header(AUTH_HEADER, &self.api_key)
            .query(&[("counts", "true")])
            .send()
            .map_err(|e| Self::transport_error(operation, e))?;
        Self::checked_json(operation, response)
    }

    fn entries(
        &self,
        category_id: Option<u64>,
        offset: usize,
        limit: usize,
    ) -> Result<JsonValue, String> {
        let operation = "GET /v1/entries";
        let mut request = self
            .client
            .get(format!("{}/v1/entries", self.domain))
            .header(AUTH_HEADER, &self.api_key)
            .query(&[
                ("status", "unread"),
                ("order", "published_at"),
                ("direction", "desc"),
            ])
            .query(&[("offset", offset), ("limit", limit)]);
        if let Some(category_id) = category_id {
            request = request.query(&[("category_id", category_id)]);
        }
        let response = request
            .send()
            .map_err(|e| Self::transport_error(operation, e))?;
        Self::checked_json(operation, response)
    }

    fn entry(&self, entry_id: u64) -> Result<JsonValue, String> {
        let operation = format!("GET /v1/entries/{}", entry_id);
        let response = self
            .client
            .get(format!("{}/v1/entries/{}", self.domain, entry_id))
            .header(AUTH_HEADER, &self.api_key)
            .send()
            .map_err(|e| Self::transport_error(&operation, e))?;
        Self::checked_json(&operation, response)
    }

    fn set_status(&self, entry_id: u64, status: MinifluxStatus) -> Result<(), String> {
        let operation = format!(
            "PUT /v1/entries (entry={}, status={})",
            entry_id,
            status.as_str()
        );
        let response = self
            .client
            .put(format!("{}/v1/entries", self.domain))
            .header(AUTH_HEADER, &self.api_key)
            .header(CONTENT_TYPE, "application/json")
            .json(&json!({"entry_ids": [entry_id], "status": status.as_str()}))
            .send()
            .map_err(|e| Self::transport_error(&operation, e))?;
        let response_status = response.status();
        if response_status.is_success() {
            eprintln!(
                "[miniflux] {} returned HTTP {}.",
                operation, response_status
            );
            Ok(())
        } else {
            Self::checked_json(&operation, response).map(|_| ())
        }
    }
}

fn spawn_api_worker(
    domain: String,
    api_key: String,
    hub: &Hub,
) -> Result<Sender<ApiCommand>, String> {
    let api = Api::new(domain, api_key)?;
    let (sender, receiver) = mpsc::channel::<ApiCommand>();
    let hub = hub.clone();
    eprintln!("[miniflux] Starting API worker thread.");
    thread::spawn(move || {
        while let Ok(command) = receiver.recv() {
            let description = command.description();
            let request_id = match command {
                ApiCommand::ListCategories { request_id }
                | ApiCommand::ListEntries { request_id, .. }
                | ApiCommand::GetEntry { request_id, .. }
                | ApiCommand::SetStatus { request_id, .. } => request_id,
            };
            eprintln!(
                "[miniflux] Request {} started: {}.",
                request_id, description
            );
            let response = match command {
                ApiCommand::ListCategories { .. } => api.categories().map(|categories| {
                    json!({"type": "categories", "requestId": request_id,
                                              "categories": categories})
                }),
                ApiCommand::ListEntries {
                    category_id,
                    offset,
                    limit,
                    ..
                } => api.entries(category_id, offset, limit).map(|result| {
                    json!({"type": "entries", "requestId": request_id,
                                            "result": result})
                }),
                ApiCommand::GetEntry { entry_id, .. } => api.entry(entry_id).map(|entry| {
                    json!({"type": "entry", "requestId": request_id,
                                        "entry": entry})
                }),
                ApiCommand::SetStatus {
                    entry_id, status, ..
                } => api.set_status(entry_id, status).map(|_| {
                    json!({"type": "status", "requestId": request_id,
                                    "entryId": entry_id, "status": status.as_str()})
                }),
            };
            let response = match response {
                Ok(response) => {
                    eprintln!(
                        "[miniflux] Request {} completed: {}.",
                        request_id, description
                    );
                    response
                }
                Err(message) => {
                    eprintln!(
                        "[miniflux] Request {} failed ({}): {}.",
                        request_id, description, message
                    );
                    json!({"type": "error", "requestId": request_id, "message": message})
                }
            };
            if hub.send(Event::MinifluxResponse(response)).is_err() {
                eprintln!(
                    "[miniflux] Request {} response could not be delivered; event channel closed.",
                    request_id
                );
                break;
            }
        }
        eprintln!("[miniflux] API worker thread stopped.");
    });
    Ok(sender)
}

pub fn entry_as_html(entry: &MinifluxEntry) -> String {
    fn escape(value: &str) -> String {
        value
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    }

    let title = escape(&entry.title);
    let author = escape(&entry.author);
    let feed = escape(&entry.feed.title);
    let url = escape(&entry.url);
    let byline = match (author.is_empty(), feed.is_empty()) {
        (false, false) => format!("{} — {}", author, feed),
        (false, true) => author,
        (true, false) => feed,
        (true, true) => String::new(),
    };
    let source = if url.is_empty() {
        String::new()
    } else {
        format!("<p><a href=\"{}\">Open original article</a></p>", url)
    };
    format!("<!doctype html><html><head><meta charset=\"utf-8\"><title>{}</title></head><body><h1>{}</h1><p><em>{}</em></p>{}{}</body></html>",
            title, title, byline, entry.content, source)
}

pub struct Miniflux {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    worker: Option<Sender<ApiCommand>>,
    categories: Vec<MinifluxCategory>,
    entries: Vec<MinifluxEntry>,
    category_id: Option<u64>,
    current_page: usize,
    pages_count: usize,
    total: usize,
    per_page: usize,
    next_request_id: u64,
    categories_request: Option<u64>,
    entries_request: Option<u64>,
    entry_request: Option<u64>,
    status_requests: HashMap<u64, MinifluxStatus>,
    configured: bool,
}

impl Miniflux {
    pub fn new(
        rect: Rectangle,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> Miniflux {
        let id = ID_FEEDER.next();
        let per_page = Self::per_page(rect);
        let mut app = Miniflux {
            id,
            rect,
            children: Vec::new(),
            worker: None,
            categories: Vec::new(),
            entries: Vec::new(),
            category_id: None,
            current_page: 0,
            pages_count: 1,
            total: 0,
            per_page,
            next_request_id: 1,
            categories_request: None,
            entries_request: None,
            entry_request: None,
            status_requests: HashMap::new(),
            configured: false,
        };
        app.rebuild(rq, context);

        let settings = &context.settings.miniflux;
        eprintln!("[miniflux] Opening app (online={}, wifi={}, domain_configured={}, api_key_configured={}).",
                  context.online, context.settings.wifi,
                  !settings.domain.trim().is_empty(), !settings.api_key.trim().is_empty());
        if settings.domain.trim().is_empty() || settings.api_key.trim().is_empty() {
            eprintln!("[miniflux] Configuration is incomplete; refusing to start API worker.");
            hub.send(Event::Notify(
                "Configure [miniflux] in Settings.toml.".to_string(),
            ))
            .ok();
            return app;
        }

        match spawn_api_worker(settings.domain.clone(), settings.api_key.clone(), hub) {
            Ok(worker) => {
                eprintln!("[miniflux] API worker initialized.");
                app.worker = Some(worker);
                app.configured = true;
                if context.online {
                    eprintln!("[miniflux] Network is online; requesting initial data.");
                    app.refresh();
                } else {
                    eprintln!("[miniflux] Network is offline; waiting for NetUp event.");
                    hub.send(Event::Notify(
                        "Waiting for a network connection.".to_string(),
                    ))
                    .ok();
                    if !context.settings.wifi {
                        eprintln!("[miniflux] Requesting WiFi enable.");
                        hub.send(Event::SetWifi(true)).ok();
                    }
                }
            }
            Err(message) => {
                eprintln!("[miniflux] API worker initialization failed: {}.", message);
                hub.send(Event::Notify(format!(
                    "Can't initialize Miniflux: {}.",
                    message
                )))
                .ok();
            }
        }
        app
    }

    fn send(&mut self, command: ApiCommand) {
        let description = command.description();
        if let Some(worker) = self.worker.as_ref() {
            if worker.send(command).is_err() {
                eprintln!(
                    "[miniflux] Can't queue API command; worker stopped: {}.",
                    description
                );
                self.configured = false;
            }
        } else {
            eprintln!(
                "[miniflux] Can't queue API command; worker is unavailable: {}.",
                description
            );
        }
    }

    fn request_id(&mut self) -> u64 {
        let id = self.next_request_id;
        self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
        id
    }

    fn refresh(&mut self) {
        if !self.configured {
            eprintln!("[miniflux] Refresh ignored because the API worker is not configured.");
            return;
        }
        let categories_request = self.request_id();
        self.categories_request = Some(categories_request);
        self.send(ApiCommand::ListCategories {
            request_id: categories_request,
        });
        self.request_entries();
    }

    fn request_entries(&mut self) {
        if !self.configured {
            eprintln!("[miniflux] Entry request ignored because the API worker is not configured.");
            return;
        }
        let request_id = self.request_id();
        self.entries_request = Some(request_id);
        self.send(ApiCommand::ListEntries {
            request_id,
            category_id: self.category_id,
            offset: self.current_page * self.per_page,
            limit: self.per_page,
        });
    }

    fn request_entry(&mut self, entry_id: u64) {
        let request_id = self.request_id();
        self.entry_request = Some(request_id);
        self.send(ApiCommand::GetEntry {
            request_id,
            entry_id,
        });
    }

    fn set_status(&mut self, entry_id: u64, status: MinifluxStatus) {
        let request_id = self.request_id();
        self.status_requests.insert(request_id, status);
        self.send(ApiCommand::SetStatus {
            request_id,
            entry_id,
            status,
        });
    }

    fn per_page(rect: Rectangle) -> usize {
        let dpi = CURRENT_DEVICE.dpi;
        let bar_height = scale_by_dpi(SMALL_BAR_HEIGHT, dpi) as i32;
        let row_height = scale_by_dpi(BIG_BAR_HEIGHT, dpi) as i32;
        ((rect.height() as i32 - 2 * bar_height) / row_height).max(1) as usize
    }

    fn category_title(&self) -> String {
        self.category_id
            .and_then(|id| self.categories.iter().find(|category| category.id == id))
            .map(|category| category.title.clone())
            .unwrap_or_else(|| "All Categories".to_string())
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
            format!("Miniflux — {}", self.category_title()),
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

        let content_rect = rect![
            self.rect.min.x,
            self.rect.min.y + side + big_thickness,
            self.rect.max.x,
            self.rect.max.y - side - small_thickness
        ];
        if self.entries.is_empty() {
            self.children.push(Box::new(Label::new(
                content_rect,
                "No unread entries".to_string(),
                Align::Center,
            )));
        } else {
            let heights = crate::geom::divide(content_rect.height() as i32, self.per_page as i32);
            let mut y = content_rect.min.y;
            for (index, entry) in self.entries.iter().cloned().enumerate() {
                let row = EntryRow::new(
                    rect![
                        content_rect.min.x,
                        y,
                        content_rect.max.x,
                        y + heights[index]
                    ],
                    entry,
                );
                self.children.push(Box::new(row));
                y += heights[index];
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
                self.rect.max.y - side + big_thickness
            ],
            BLACK,
        )));
        let bottom = MinifluxBottomBar::new(
            rect![
                self.rect.min.x,
                self.rect.max.y - side + big_thickness,
                self.rect.max.x,
                self.rect.max.y
            ],
            self.current_page,
            self.pages_count,
            self.total,
        );
        self.children.push(Box::new(bottom));
        rq.add(RenderData::new(self.id, self.rect, UpdateMode::Full));
    }

    fn toggle_title_menu(&mut self, rect: Rectangle, rq: &mut RenderQueue, context: &mut Context) {
        if let Some(index) = locate_by_id(self, ViewId::TitleMenu) {
            rq.add(RenderData::expose(
                *self.child(index).rect(),
                UpdateMode::Gui,
            ));
            self.children.remove(index);
            return;
        }
        let mut entries = vec![
            EntryKind::RadioButton(
                "All Categories".to_string(),
                EntryId::MinifluxCategory(None),
                self.category_id.is_none(),
            ),
            EntryKind::Separator,
        ];
        entries.extend(self.categories.iter().map(|category| {
            EntryKind::RadioButton(
                format!("{} ({})", category.title, category.total_unread),
                EntryId::MinifluxCategory(Some(category.id)),
                self.category_id == Some(category.id),
            )
        }));
        entries.push(EntryKind::Separator);
        entries.push(EntryKind::Command(
            "Refresh".to_string(),
            EntryId::MinifluxRefresh,
        ));
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
        response: &JsonValue,
        hub: &Hub,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) {
        let request_id = response
            .get("requestId")
            .and_then(JsonValue::as_u64)
            .unwrap_or(0);
        let response_type = response
            .get("type")
            .and_then(JsonValue::as_str)
            .unwrap_or("missing-type");
        eprintln!(
            "[miniflux] Handling response {} of type '{}'.",
            request_id, response_type
        );
        match response.get("type").and_then(JsonValue::as_str) {
            Some("categories") if self.categories_request == Some(request_id) => {
                self.categories_request = None;
                if let Some(value) = response.get("categories") {
                    match serde_json::from_value::<Vec<MinifluxCategory>>(value.clone()) {
                        Ok(categories) => {
                            eprintln!("[miniflux] Parsed {} categories.", categories.len());
                            self.categories = categories;
                        }
                        Err(err) => {
                            eprintln!(
                                "[miniflux] Can't parse categories response {}: {}.",
                                request_id, err
                            );
                            hub.send(Event::Notify(format!(
                                "Invalid Miniflux categories: {}.",
                                err
                            )))
                            .ok();
                        }
                    }
                } else {
                    eprintln!(
                        "[miniflux] Categories response {} has no 'categories' field.",
                        request_id
                    );
                }
            }
            Some("entries") if self.entries_request == Some(request_id) => {
                self.entries_request = None;
                if let Some(value) = response.get("result") {
                    match serde_json::from_value::<EntriesResult>(value.clone()) {
                        Ok(result) => {
                            eprintln!(
                                "[miniflux] Parsed {} of {} unread entries for response {}.",
                                result.entries.len(),
                                result.total,
                                request_id
                            );
                            self.total = result.total;
                            self.entries = result.entries;
                            self.pages_count = self.total.max(1).div_ceil(self.per_page);
                            if self.current_page >= self.pages_count {
                                self.current_page = self.pages_count - 1;
                                self.request_entries();
                            }
                            self.rebuild(rq, context);
                        }
                        Err(err) => {
                            eprintln!(
                                "[miniflux] Can't parse entries response {}: {}.",
                                request_id, err
                            );
                            hub.send(Event::Notify(format!("Invalid Miniflux entries: {}.", err)))
                                .ok();
                        }
                    };
                } else {
                    eprintln!(
                        "[miniflux] Entries response {} has no 'result' field.",
                        request_id
                    );
                }
            }
            Some("entry") if self.entry_request == Some(request_id) => {
                self.entry_request = None;
                if let Some(value) = response.get("entry") {
                    match serde_json::from_value::<MinifluxEntry>(value.clone()) {
                        Ok(entry) => {
                            eprintln!(
                                "[miniflux] Parsed entry {} (content_bytes={}).",
                                entry.id,
                                entry.content.len()
                            );
                            hub.send(Event::OpenMinifluxEntry(Box::new(entry))).ok();
                        }
                        Err(err) => {
                            eprintln!(
                                "[miniflux] Can't parse entry response {}: {}.",
                                request_id, err
                            );
                            hub.send(Event::Notify(format!("Invalid Miniflux entry: {}.", err)))
                                .ok();
                        }
                    }
                } else {
                    eprintln!(
                        "[miniflux] Entry response {} has no 'entry' field.",
                        request_id
                    );
                }
            }
            Some("status") => {
                if let Some(status) = self.status_requests.remove(&request_id) {
                    eprintln!(
                        "[miniflux] Status update response {} succeeded: {}.",
                        request_id,
                        status.as_str()
                    );
                    if status == MinifluxStatus::Unread {
                        hub.send(Event::Notify("Marked entry as unread.".to_string()))
                            .ok();
                    }
                } else {
                    eprintln!(
                        "[miniflux] Received unknown status response {}.",
                        request_id
                    );
                }
            }
            Some("error") => {
                self.status_requests.remove(&request_id);
                let message = response
                    .get("message")
                    .and_then(JsonValue::as_str)
                    .unwrap_or("Unknown API error");
                eprintln!(
                    "[miniflux] API response {} reported an error: {}.",
                    request_id, message
                );
                hub.send(Event::Notify(format!("Miniflux: {}.", message)))
                    .ok();
            }
            _ => {
                eprintln!("[miniflux] Ignoring stale or unknown response {} of type '{}'; expected categories={:?}, entries={:?}, entry={:?}.",
                          request_id, response_type, self.categories_request,
                          self.entries_request, self.entry_request);
            }
        }
    }
}

impl View for Miniflux {
    fn handle_event(
        &mut self,
        evt: &Event,
        hub: &Hub,
        _bus: &mut Bus,
        rq: &mut RenderQueue,
        context: &mut Context,
    ) -> bool {
        match *evt {
            Event::MinifluxResponse(ref response) => {
                self.handle_response(response, hub, rq, context);
                true
            }
            Event::MinifluxOpen(entry_id) => {
                self.request_entry(entry_id);
                true
            }
            Event::MinifluxSetStatus(entry_id, status) => {
                self.set_status(entry_id, status);
                true
            }
            Event::Select(EntryId::MinifluxCategory(category_id)) => {
                self.category_id = category_id;
                self.current_page = 0;
                self.request_entries();
                self.toggle_title_menu(Rectangle::default(), rq, context);
                true
            }
            Event::Select(EntryId::MinifluxRefresh) => {
                self.refresh();
                self.toggle_title_menu(Rectangle::default(), rq, context);
                true
            }
            Event::Page(direction) => {
                let next = match direction {
                    CycleDir::Previous => self.current_page.saturating_sub(1),
                    CycleDir::Next => (self.current_page + 1).min(self.pages_count - 1),
                };
                if next != self.current_page {
                    self.current_page = next;
                    self.request_entries();
                }
                true
            }
            Event::Device(DeviceEvent::NetUp) => {
                eprintln!("[miniflux] Received network-up event; refreshing.");
                self.refresh();
                true
            }
            Event::Reseed => {
                eprintln!("[miniflux] App became visible again; refreshing.");
                self.refresh();
                self.rebuild(rq, context);
                true
            }
            Event::ToggleNear(ViewId::TitleMenu, rect) => {
                self.toggle_title_menu(rect, rq, context);
                true
            }
            Event::ToggleNear(ViewId::MainMenu, rect) => {
                toggle_main_menu(self, rect, None, rq, context);
                true
            }
            Event::ToggleNear(ViewId::BatteryMenu, rect) => {
                toggle_battery_menu(self, rect, None, rq, context);
                true
            }
            Event::ToggleNear(ViewId::ClockMenu, rect) => {
                toggle_clock_menu(self, rect, None, rq, context);
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
        self.per_page = Self::per_page(rect);
        self.current_page = 0;
        self.request_entries();
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
        Some(ViewId::Miniflux)
    }
}

struct EntryRow {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
    entry: MinifluxEntry,
    active: bool,
}

impl EntryRow {
    fn new(rect: Rectangle, entry: MinifluxEntry) -> EntryRow {
        EntryRow {
            id: ID_FEEDER.next(),
            rect,
            children: Vec::new(),
            entry,
            active: false,
        }
    }
}

impl View for EntryRow {
    fn handle_event(
        &mut self,
        evt: &Event,
        hub: &Hub,
        _bus: &mut Bus,
        rq: &mut RenderQueue,
        _context: &mut Context,
    ) -> bool {
        match *evt {
            Event::Gesture(GestureEvent::Tap(center)) if self.rect.includes(center) => {
                self.active = true;
                rq.add(RenderData::new(self.id, self.rect, UpdateMode::Gui));
                hub.send(Event::MinifluxOpen(self.entry.id)).ok();
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
        let padding = {
            let title_font = font_from_style(fonts, &MD_TITLE, dpi);
            let padding = title_font.em() as i32;
            let width = self.rect.width() as i32 - 2 * padding;
            let mut title = title_font.plan(&self.entry.title, None, None);
            title_font.crop_right(&mut title, width);
            title_font.render(
                fb,
                scheme[1],
                &title,
                pt!(self.rect.min.x + padding, self.rect.center().y),
            );
            padding
        };
        let width = self.rect.width() as i32 - 2 * padding;
        let detail = match (
            self.entry.feed.title.is_empty(),
            self.entry.author.is_empty(),
        ) {
            (false, false) => format!("{} — {}", self.entry.feed.title, self.entry.author),
            (false, true) => self.entry.feed.title.clone(),
            (true, false) => self.entry.author.clone(),
            _ => String::new(),
        };
        {
            let detail_font = font_from_style(fonts, &MD_AUTHOR, dpi);
            let mut detail = detail_font.plan(&detail, None, None);
            detail_font.crop_right(&mut detail, width);
            detail_font.render(
                fb,
                scheme[1],
                &detail,
                pt!(self.rect.min.x + padding, self.rect.max.y - padding / 2),
            );
        }
        let date = self
            .entry
            .published_at
            .get(..10)
            .unwrap_or(&self.entry.published_at);
        let date_font = font_from_style(fonts, &NORMAL_STYLE, dpi);
        let date_plan = date_font.plan(date, None, None);
        date_font.render(
            fb,
            scheme[1],
            &date_plan,
            pt!(
                self.rect.max.x - padding - date_plan.width,
                self.rect.max.y - padding / 2
            ),
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

struct MinifluxBottomBar {
    id: Id,
    rect: Rectangle,
    children: Vec<Box<dyn View>>,
}

impl MinifluxBottomBar {
    fn new(rect: Rectangle, current_page: usize, pages_count: usize, total: usize) -> Self {
        let id = ID_FEEDER.next();
        let side = rect.height() as i32;
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
        let next: Box<dyn View> = if current_page + 1 < pages_count {
            Box::new(Icon::new(
                "arrow-right",
                next_rect,
                Event::Page(CycleDir::Next),
            ))
        } else {
            Box::new(Filler::new(next_rect, WHITE))
        };
        let label_rect = rect![rect.min.x + side, rect.min.y, rect.center().x, rect.max.y];
        let page_rect = rect![rect.center().x, rect.min.y, rect.max.x - side, rect.max.y];
        let children: Vec<Box<dyn View>> = vec![
            prev,
            Box::new(Label::new(
                label_rect,
                format!("{} unread", total),
                Align::Center,
            )),
            Box::new(PageLabel::new(page_rect, current_page, pages_count, false)),
            next,
        ];
        Self { id, rect, children }
    }
}

impl View for MinifluxBottomBar {
    fn handle_event(
        &mut self,
        evt: &Event,
        _hub: &Hub,
        bus: &mut Bus,
        _rq: &mut RenderQueue,
        _context: &mut Context,
    ) -> bool {
        match *evt {
            Event::Gesture(GestureEvent::Swipe {
                dir: Dir::West,
                start,
                ..
            }) if self.rect.includes(start) => {
                bus.push_back(Event::Page(CycleDir::Next));
                true
            }
            Event::Gesture(GestureEvent::Swipe {
                dir: Dir::East,
                start,
                ..
            }) if self.rect.includes(start) => {
                bus.push_back(Event::Page(CycleDir::Previous));
                true
            }
            _ => false,
        }
    }
    fn render(&self, _fb: &mut dyn Framebuffer, _rect: Rectangle, _fonts: &mut Fonts) {}
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
    use std::io::{Read, Write};
    use std::net::TcpListener;

    fn server(status: &str, body: &str) -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let status = status.to_string();
        let body = body.to_string();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new();
            let mut buf = [0; 4096];
            loop {
                let count = stream.read(&mut buf).unwrap();
                request.extend_from_slice(&buf[..count]);
                let header_end = request.windows(4).position(|part| part == b"\r\n\r\n");
                if count == 0 {
                    break;
                }
                if let Some(header_end) = header_end {
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .and_then(|value| value.parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if request.len() >= header_end + 4 + content_length {
                        break;
                    }
                }
            }
            write!(stream,
                   "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                   status, body.len(), body).unwrap();
            String::from_utf8(request).unwrap()
        });
        (format!("http://{}", address), handle)
    }

    #[test]
    fn wraps_entry_content_and_escapes_metadata() {
        let entry = MinifluxEntry {
            title: "A <title>".to_string(),
            url: "https://example.com/?a=1&b=2".to_string(),
            content: "<p>Body</p>".to_string(),
            ..Default::default()
        };
        let html = entry_as_html(&entry);
        assert!(html.contains("<title>A &lt;title&gt;</title>"));
        assert!(html.contains("<p>Body</p>"));
        assert!(html.contains("a=1&amp;b=2"));
    }

    #[test]
    fn lists_unread_entries_for_a_category() {
        let (domain, request) = server("200 OK", r#"{"total":0,"entries":[]}"#);
        let api = Api::new(format!("{}/", domain), "secret".to_string()).unwrap();
        let result = api.entries(Some(42), 10, 5).unwrap();
        assert_eq!(result["total"], 0);

        let request = request.join().unwrap().to_ascii_lowercase();
        assert!(request.starts_with("get /v1/entries?"));
        assert!(request.contains("status=unread"));
        assert!(request.contains("category_id=42"));
        assert!(request.contains("offset=10"));
        assert!(request.contains("limit=5"));
        assert!(request.contains("x-auth-token: secret"));
    }

    #[test]
    fn updates_entry_status() {
        let (domain, request) = server("204 No Content", "");
        let api = Api::new(domain, "secret".to_string()).unwrap();
        api.set_status(123, MinifluxStatus::Unread).unwrap();

        let request = request.join().unwrap();
        assert!(request.starts_with("PUT /v1/entries "));
        assert!(request.contains(r#"{"entry_ids":[123],"status":"unread"}"#));
    }
}
