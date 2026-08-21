use reqwest::blocking::{Client, Response};
use reqwest::header::CONTENT_TYPE;
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::{self, BufRead};
use std::time::Duration;

const AUTH_HEADER: &str = "X-Auth-Token";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
enum Command {
    Configure {
        domain: String,
        api_key: String,
    },
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
        status: String,
    },
}

struct Api {
    client: Client,
    domain: String,
    api_key: String,
}

impl Api {
    fn new(domain: String, api_key: String) -> Result<Self, String> {
        let client = Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            client,
            domain: domain.trim_end_matches('/').to_string(),
            api_key,
        })
    }

    fn get(&self, path: &str) -> Result<Response, String> {
        self.client
            .get(format!("{}{}", self.domain, path))
            .header(AUTH_HEADER, &self.api_key)
            .send()
            .map_err(|e| e.to_string())
    }

    fn checked_json(response: Response) -> Result<Value, String> {
        let status = response.status();
        if status.is_success() {
            response.json().map_err(|e| e.to_string())
        } else {
            let reason = status.canonical_reason().unwrap_or("HTTP error");
            let message = response
                .json::<Value>()
                .ok()
                .and_then(|body| {
                    body.get("error_message")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .unwrap_or_else(|| format!("{} {}", status.as_u16(), reason));
            Err(message)
        }
    }

    fn categories(&self) -> Result<Value, String> {
        let response = self
            .client
            .get(format!("{}/v1/categories", self.domain))
            .header(AUTH_HEADER, &self.api_key)
            .query(&[("counts", "true")])
            .send()
            .map_err(|e| e.to_string())?;
        Self::checked_json(response)
    }

    fn entries(
        &self,
        category_id: Option<u64>,
        offset: usize,
        limit: usize,
    ) -> Result<Value, String> {
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
        let response = request.send().map_err(|e| e.to_string())?;
        Self::checked_json(response)
    }

    fn entry(&self, entry_id: u64) -> Result<Value, String> {
        Self::checked_json(self.get(&format!("/v1/entries/{}", entry_id))?)
    }

    fn set_status(&self, entry_id: u64, status: &str) -> Result<(), String> {
        if status != "read" && status != "unread" {
            return Err("invalid entry status".to_string());
        }
        let response = self
            .client
            .put(format!("{}/v1/entries", self.domain))
            .header(AUTH_HEADER, &self.api_key)
            .header(CONTENT_TYPE, "application/json")
            .json(&json!({"entry_ids": [entry_id], "status": status}))
            .send()
            .map_err(|e| e.to_string())?;
        if response.status().is_success() {
            Ok(())
        } else {
            Self::checked_json(response).map(|_| ())
        }
    }
}

fn emit(value: Value) {
    println!("{}", value);
}

fn main() {
    let stdin = io::stdin();
    let mut api: Option<Api> = None;

    for line in stdin.lock().lines() {
        let line = match line {
            Ok(line) => line,
            Err(err) => {
                eprintln!("Can't read command: {}.", err);
                break;
            }
        };
        let command = match serde_json::from_str::<Command>(&line) {
            Ok(command) => command,
            Err(err) => {
                emit(
                    json!({"type": "error", "requestId": 0, "message": format!("Invalid command: {}", err)}),
                );
                continue;
            }
        };

        if let Command::Configure { domain, api_key } = command {
            match Api::new(domain, api_key) {
                Ok(value) => api = Some(value),
                Err(message) => emit(json!({"type": "error", "requestId": 0, "message": message})),
            }
            continue;
        }

        let request_id = match command {
            Command::ListCategories { request_id }
            | Command::ListEntries { request_id, .. }
            | Command::GetEntry { request_id, .. }
            | Command::SetStatus { request_id, .. } => request_id,
            Command::Configure { .. } => unreachable!(),
        };
        let Some(api) = api.as_ref() else {
            emit(
                json!({"type": "error", "requestId": request_id, "message": "Miniflux is not configured"}),
            );
            continue;
        };

        let response = match command {
            Command::ListCategories { .. } => api.categories()
                .map(|categories| json!({"type": "categories", "requestId": request_id, "categories": categories})),
            Command::ListEntries { category_id, offset, limit, .. } => api.entries(category_id, offset, limit)
                .map(|result| json!({"type": "entries", "requestId": request_id, "result": result})),
            Command::GetEntry { entry_id, .. } => api.entry(entry_id)
                .map(|entry| json!({"type": "entry", "requestId": request_id, "entry": entry})),
            Command::SetStatus { entry_id, ref status, .. } => api.set_status(entry_id, status)
                .map(|_| json!({"type": "status", "requestId": request_id, "entryId": entry_id, "status": status})),
            Command::Configure { .. } => unreachable!(),
        };

        match response {
            Ok(value) => emit(value),
            Err(message) => {
                emit(json!({"type": "error", "requestId": request_id, "message": message}))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

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
                if count == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..count]);
                let header_end = request.windows(4).position(|part| part == b"\r\n\r\n");
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
        api.set_status(123, "unread").unwrap();

        let request = request.join().unwrap();
        assert!(request.starts_with("PUT /v1/entries "));
        assert!(request.contains(r#"{"entry_ids":[123],"status":"unread"}"#));
    }

    #[test]
    fn returns_miniflux_error_messages() {
        let (domain, request) = server("401 Unauthorized", r#"{"error_message":"bad key"}"#);
        let api = Api::new(domain, "secret".to_string()).unwrap();
        assert_eq!(api.categories().unwrap_err(), "bad key");
        request.join().unwrap();
    }
}
