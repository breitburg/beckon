// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 breitburg

//! Mail toolset — searches the user's locally-synced mail.
//!
//! elementary Mail stores messages through Evolution Data Server's Camel
//! backend, which has no IPC/search API (Camel is an in-process library). The
//! only programmatic route is Camel's per-account summary cache, a SQLite file
//! at `~/.cache/evolution/mail/<account-uid>/folders.db`. Each folder is a table
//! (named by its full name, e.g. `INBOX`, `[Gmail]/Sent Mail`) of message
//! summaries: `subject`, `mail_from`, `mail_to`, `mail_cc`, `dsent`/`dreceived`
//! (Unix seconds), and flags. We open it read-only and immutable so we never
//! disturb the writer.
//!
//! Search covers the header fields (subject, sender, recipients) plus whatever
//! body `preview` Camel happened to cache — not full message bodies, which are
//! stored separately and largely not synced. Only locally-cached mail is
//! visible.

use std::path::{Path, PathBuf};

use gtk4::glib;
use rusqlite::types::Value;
use rusqlite::{Connection, OpenFlags};
use serde_json::json;

use crate::datetime::{format_epoch, parse_iso};
use crate::tools::{truncate, Tool, MAX_OUTPUT_BYTES};

const DEFAULT_LIMIT: usize = 20;
const MAX_LIMIT: usize = 100;

pub fn tools() -> Vec<Tool> {
    vec![search_tool()]
}

struct Account {
    name: String,
    db: PathBuf,
}

struct Message {
    subject: String,
    from: String,
    folder: String,
    account: String,
    date: i64,
    unread: bool,
    attachment: bool,
}

/// Mail accounts with a Camel summary cache under `~/.cache/evolution/mail`.
fn discover_accounts() -> Vec<Account> {
    let base = glib::user_cache_dir().join("evolution/mail");
    let Ok(entries) = std::fs::read_dir(&base) else {
        return Vec::new();
    };
    let mut accounts = Vec::new();
    for entry in entries.flatten() {
        let db = entry.path().join("folders.db");
        if !db.is_file() {
            continue;
        }
        let uid = entry.file_name().to_string_lossy().into_owned();
        let name = account_name(&uid).unwrap_or(uid);
        accounts.push(Account { name, db });
    }
    accounts
}

/// Human account name from the EDS `.source` file, falling back to the UID.
fn account_name(uid: &str) -> Option<String> {
    let path = glib::user_config_dir().join(format!("evolution/sources/{uid}.source"));
    let content = std::fs::read_to_string(path).ok()?;
    crate::eds::ini_value(&content, "DisplayName")
}

/// Open a Camel cache read-only and immutable (no locking; never perturbs EDS).
fn open_db(path: &Path) -> Result<Connection, String> {
    let uri = format!("file:{}?mode=ro&immutable=1", path.to_string_lossy());
    Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|err| format!("open {}: {err}", path.display()))
}

/// Folder names listed in the cache's `folders` table.
fn folder_names(conn: &Connection) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare("SELECT folder_name FROM folders")
        .map_err(|err| err.to_string())?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|err| err.to_string())?;
    Ok(rows.filter_map(Result::ok).collect())
}

/// A folder name as a quoted SQL identifier (Camel names the table after it).
fn quote_ident(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Wrap a query for a `LIKE ? ESCAPE '\'` match, escaping wildcards so the text
/// is treated literally.
fn like_pattern(query: &str) -> String {
    let mut out = String::with_capacity(query.len() + 2);
    out.push('%');
    for c in query.chars() {
        if matches!(c, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('%');
    out
}

fn search_tool() -> Tool {
    Tool::new(
        "mail_search_messages",
        "Search the user's locally-synced mail (elementary Mail / Evolution) by text and/or \
         date range. Matches the subject, sender, and recipients across all folders and returns \
         each message's sender, subject, folder, and date. Read-only; it does not open full \
         message bodies, and only mail already synced to this device is searchable.",
        json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Text to find in subject, sender, or recipients (case-insensitive). Omit to match everything in the date range."},
                "start": {"type": "string", "description": "Only messages on/after this instant, ISO 8601 (YYYY-MM-DD or YYYY-MM-DDTHH:MM:SSZ), interpreted as UTC."},
                "end": {"type": "string", "description": "Only messages before this instant, ISO 8601 UTC. A date-only value includes that whole day."},
                "folder": {"type": "string", "description": "Restrict to folders whose name contains this substring (e.g. \"Inbox\", \"Sent\")."},
                "unread_only": {"type": "boolean", "description": "Only unread messages."},
                "limit": {"type": "integer", "description": "Maximum results (default 20, max 100)."}
            },
            "additionalProperties": false
        }),
        |args| {
            let query = args["query"].as_str().filter(|s| !s.is_empty());
            let start = match args["start"].as_str() {
                Some(s) => Some(parse_iso(s)?.0),
                None => None,
            };
            // A date-only `end` should include that whole day (we filter `< end`).
            let end = match args["end"].as_str() {
                Some(s) => {
                    let (secs, has_time) = parse_iso(s)?;
                    Some(if has_time { secs } else { secs + 86_400 })
                }
                None => None,
            };
            let folder_filter = args["folder"].as_str().map(|s| s.to_lowercase());
            let unread_only = args["unread_only"].as_bool().unwrap_or(false);
            let limit = args["limit"]
                .as_u64()
                .map(|n| (n as usize).clamp(1, MAX_LIMIT))
                .unwrap_or(DEFAULT_LIMIT);

            let accounts = discover_accounts();
            if accounts.is_empty() {
                return Ok("No local mail store found (the Evolution/elementary Mail cache is \
                           empty — open Mail and let it sync first)."
                    .to_string());
            }
            let multi_account = accounts.len() > 1;

            let mut messages = Vec::new();
            let mut notes = Vec::new();
            for account in &accounts {
                let conn = match open_db(&account.db) {
                    Ok(conn) => conn,
                    Err(err) => {
                        notes.push(format!("- {}: {err}", account.name));
                        continue;
                    }
                };
                let folders = match folder_names(&conn) {
                    Ok(folders) => folders,
                    Err(err) => {
                        notes.push(format!("- {}: {err}", account.name));
                        continue;
                    }
                };
                for folder in folders {
                    if let Some(f) = &folder_filter {
                        if !folder.to_lowercase().contains(f.as_str()) {
                            continue;
                        }
                    }
                    if let Err(err) = collect_folder(
                        &conn, account, &folder, query, start, end, unread_only, limit,
                        &mut messages,
                    ) {
                        notes.push(format!("- {} / {folder}: {err}", account.name));
                    }
                }
            }

            messages.sort_by(|a, b| b.date.cmp(&a.date));
            messages.truncate(limit);

            let mut out = String::new();
            if messages.is_empty() {
                out.push_str("No matching messages found.");
            } else {
                out.push_str(&format!("{} message(s):\n\n", messages.len()));
                for msg in &messages {
                    out.push_str(&render(msg, multi_account));
                }
            }
            if !notes.is_empty() {
                out.push_str("\nNotes (sources that could not be read):\n");
                out.push_str(&notes.join("\n"));
            }
            truncate(&mut out, MAX_OUTPUT_BYTES);
            Ok(out)
        },
    )
}

/// Query one folder and append its matches to `messages`.
#[allow(clippy::too_many_arguments)]
fn collect_folder(
    conn: &Connection,
    account: &Account,
    folder: &str,
    query: Option<&str>,
    start: Option<i64>,
    end: Option<i64>,
    unread_only: bool,
    limit: usize,
    messages: &mut Vec<Message>,
) -> Result<(), String> {
    let mut clauses = vec!["deleted = 0".to_string()];
    let mut params: Vec<Value> = Vec::new();
    if unread_only {
        clauses.push("read = 0".into());
    }
    if let Some(s) = start {
        clauses.push("COALESCE(dreceived, dsent, 0) >= ?".into());
        params.push(Value::Integer(s));
    }
    if let Some(e) = end {
        clauses.push("COALESCE(dreceived, dsent, 0) < ?".into());
        params.push(Value::Integer(e));
    }
    if let Some(q) = query {
        clauses.push(
            "(subject LIKE ? ESCAPE '\\' OR mail_from LIKE ? ESCAPE '\\' \
             OR mail_to LIKE ? ESCAPE '\\' OR mail_cc LIKE ? ESCAPE '\\' \
             OR IFNULL(preview, '') LIKE ? ESCAPE '\\')"
                .into(),
        );
        let pattern = like_pattern(q);
        for _ in 0..5 {
            params.push(Value::Text(pattern.clone()));
        }
    }
    params.push(Value::Integer(limit as i64));

    let sql = format!(
        "SELECT IFNULL(subject, ''), IFNULL(mail_from, ''), \
         COALESCE(dreceived, dsent, 0), read, attachment \
         FROM {table} WHERE {where_clause} ORDER BY COALESCE(dreceived, dsent, 0) DESC LIMIT ?",
        table = quote_ident(folder),
        where_clause = clauses.join(" AND "),
    );

    let mut stmt = conn.prepare(&sql).map_err(|err| err.to_string())?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(params), |row| {
            Ok(Message {
                subject: row.get(0)?,
                from: row.get(1)?,
                date: row.get(2)?,
                unread: row.get::<_, i64>(3)? == 0,
                attachment: row.get::<_, i64>(4)? != 0,
                folder: folder.to_string(),
                account: account.name.clone(),
            })
        })
        .map_err(|err| err.to_string())?;
    for msg in rows.flatten() {
        messages.push(msg);
    }
    Ok(())
}

fn render(msg: &Message, show_account: bool) -> String {
    let subject = if msg.subject.is_empty() {
        "(no subject)"
    } else {
        &msg.subject
    };
    let mut marks = String::new();
    if msg.unread {
        marks.push_str(" ●");
    }
    if msg.attachment {
        marks.push_str(" 📎");
    }
    let location = if show_account {
        format!("{} / {}", msg.account, msg.folder)
    } else {
        msg.folder.clone()
    };
    format!(
        "• {subject}{marks}\n    {}  ·  {location}  ·  {}\n",
        msg.from,
        format_epoch(msg.date)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn like_pattern_escapes_wildcards() {
        assert_eq!(like_pattern("a%b_c"), "%a\\%b\\_c%");
        assert_eq!(like_pattern("plain"), "%plain%");
    }

    #[test]
    fn quote_ident_escapes_quotes() {
        assert_eq!(quote_ident("INBOX"), "\"INBOX\"");
        assert_eq!(quote_ident("[Gmail]/Sent Mail"), "\"[Gmail]/Sent Mail\"");
        assert_eq!(quote_ident("we\"ird"), "\"we\"\"ird\"");
    }

    // Live search against the real Camel cache. Ignored by default (needs a
    // synced mail store). Run with `cargo test --release -- --ignored live_mail`.
    #[test]
    #[ignore]
    fn live_mail_search() {
        let accounts = discover_accounts();
        assert!(!accounts.is_empty(), "no mail accounts found in cache");
        let account = &accounts[0];
        let conn = open_db(&account.db).expect("open cache");
        let folders = folder_names(&conn).expect("folders");
        assert!(
            folders.iter().any(|f| f == "INBOX"),
            "expected an INBOX folder, got {folders:?}"
        );

        // No filters: INBOX should yield up to `limit` rows with sane fields.
        let mut msgs = Vec::new();
        collect_folder(&conn, account, "INBOX", None, None, None, false, 5, &mut msgs)
            .expect("query INBOX");
        assert!(!msgs.is_empty(), "INBOX returned no messages");
        assert!(msgs.iter().all(|m| m.date > 0), "messages need a date");

        // Text filter should still parse/run (may legitimately match nothing).
        let mut filtered = Vec::new();
        collect_folder(
            &conn, account, "INBOX", Some("the"), None, None, false, 5, &mut filtered,
        )
        .expect("filtered query");
    }
}
