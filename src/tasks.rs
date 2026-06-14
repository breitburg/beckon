// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 breitburg

//! Tasks toolset backed by Evolution Data Server (EDS) over D-Bus.
//!
//! elementary's Tasks app is a frontend on EDS, so the same to-dos are reachable
//! through the session-bus calendar factory's task-list side (`OpenTaskList`).
//! The D-Bus plumbing, source registry, and iCalendar text handling live in
//! [`crate::eds`]; this module adds only the VTODO-specific parse/build/render
//! and the four tools the model calls.
//!
//! Tasks are VTODO components. Completion is the trio EDS/elementary use:
//! `STATUS`, `PERCENT-COMPLETE`, and a `COMPLETED` timestamp. Dates are handled
//! as UTC with exact integer math (no `chrono`), matching the calendar toolset.

use serde_json::json;

use crate::datetime::{epoch_to_make_time, now_epoch};
use crate::eds::{self, Source};
use crate::tools::{truncate, Tool, MAX_OUTPUT_BYTES};

/// EDS source INI group identifying a task list (vs. a calendar or memo list).
const SOURCE_GROUP: &str = "[Task List]";

/// CalendarFactory method that opens a task list by source UID.
const FACTORY_METHOD: &str = "OpenTaskList";

/// UID of the built-in writable local task list; the default create/modify
/// target when the model names no list.
const DEFAULT_LIST_UID: &str = "system-task-list";

/// EDS S-expression matching every object in a list. VTODOs have no time range
/// to query against the way events do, so we fetch all and filter in Rust.
const MATCH_ALL: &str = "#t";

/// Builds the tasks tools exposed to the model. One settings toggle ("tasks")
/// enables this whole set.
pub fn tools() -> Vec<Tool> {
    vec![
        list_tasks_tool(),
        create_task_tool(),
        modify_task_tool(),
        delete_task_tool(),
    ]
}

fn list_task_sources(conn: &gtk4::gio::DBusConnection) -> Result<Vec<Source>, String> {
    eds::list_sources(conn, SOURCE_GROUP)
}

// ---------------------------------------------------------------------------
// iCalendar (RFC 5545) — VTODO parse and build
// ---------------------------------------------------------------------------

struct Task {
    summary: String,
    due_raw: Option<String>,
    due_all_day: bool,
    status: Option<String>,
    description: Option<String>,
    uid: Option<String>,
    list: String,
    sort_key: String,
}

impl Task {
    /// A task counts as done when EDS marks it `COMPLETED`.
    fn completed(&self) -> bool {
        self.status
            .as_deref()
            .is_some_and(|s| s.eq_ignore_ascii_case("COMPLETED"))
    }
}

/// Parse one VTODO's inner lines into a [`Task`] (list set by caller).
fn parse_vtodo(lines: &[String]) -> Task {
    let mut task = Task {
        summary: String::new(),
        due_raw: None,
        due_all_day: false,
        status: None,
        description: None,
        uid: None,
        list: String::new(),
        sort_key: "0".to_string(),
    };
    for line in lines {
        let Some((name, params, value)) = eds::split_property(line) else {
            continue;
        };
        match name.as_str() {
            "SUMMARY" => task.summary = eds::unescape(&value),
            "DUE" => {
                task.due_all_day = eds::is_date_only(&params, &value);
                task.due_raw = Some(value);
            }
            "STATUS" => task.status = Some(value),
            "DESCRIPTION" => task.description = Some(eds::unescape(&value)),
            "UID" => task.uid = Some(value),
            _ => {}
        }
    }
    task.sort_key = eds::sort_key(&task.due_raw);
    task
}

/// Parse every VTODO in a reply element (a bare VTODO or a VCALENDAR holding
/// one or more).
fn parse_tasks(blob: &str) -> Vec<Task> {
    eds::components(blob, "VTODO")
        .iter()
        .map(|lines| parse_vtodo(lines))
        .collect()
}

/// Build a bare VTODO for creation.
fn build_vtodo(
    uid: &str,
    summary: &str,
    due: Option<&str>,
    description: Option<&str>,
) -> Result<String, String> {
    let mut v = String::from("BEGIN:VTODO\r\n");
    v.push_str(&format!("UID:{uid}\r\n"));
    v.push_str(&format!("DTSTAMP:{}\r\n", epoch_to_make_time(now_epoch())));
    v.push_str(&format!("SUMMARY:{}\r\n", eds::escape(summary)));
    v.push_str("STATUS:NEEDS-ACTION\r\n");
    v.push_str("PERCENT-COMPLETE:0\r\n");
    if let Some(d) = due.filter(|d| !d.is_empty()) {
        v.push_str(&format!("{}\r\n", eds::dt_property("DUE", d)?));
    }
    if let Some(desc) = description.filter(|d| !d.is_empty()) {
        v.push_str(&format!("DESCRIPTION:{}\r\n", eds::escape(desc)));
    }
    v.push_str("END:VTODO\r\n");
    Ok(v)
}

/// Set the completion trio (`STATUS`/`PERCENT-COMPLETE`/`COMPLETED`) on an
/// unfolded VTODO. Completing stamps `COMPLETED` with the current UTC time;
/// reopening drops it and resets to `NEEDS-ACTION`.
fn set_completion(lines: &mut Vec<String>, completed: bool) {
    if completed {
        eds::set_property(lines, "STATUS", Some("STATUS:COMPLETED".to_string()));
        eds::set_property(lines, "PERCENT-COMPLETE", Some("PERCENT-COMPLETE:100".to_string()));
        eds::set_property(
            lines,
            "COMPLETED",
            Some(format!("COMPLETED:{}", epoch_to_make_time(now_epoch()))),
        );
    } else {
        eds::set_property(lines, "STATUS", Some("STATUS:NEEDS-ACTION".to_string()));
        eds::set_property(lines, "PERCENT-COMPLETE", Some("PERCENT-COMPLETE:0".to_string()));
        eds::set_property(lines, "COMPLETED", None);
    }
}

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

fn list_tasks_tool() -> Tool {
    Tool::new(
        "tasks_list_tasks",
        "List to-do tasks across all the user's task lists. By default only \
         open (not-yet-completed) tasks are returned. Each task shows its title, \
         due date, status, list, and uid (use the uid to modify or delete a task).",
        json!({
            "type": "object",
            "properties": {
                "include_completed": {"type": "boolean", "description": "Include completed tasks too. Defaults to false (open tasks only)."},
                "list": {"type": "string", "description": "Optional: only this task list (case-insensitive name substring)."}
            },
            "additionalProperties": false
        }),
        |args| {
            let include_completed = args["include_completed"].as_bool().unwrap_or(false);
            let filter = args["list"].as_str().map(|s| s.to_lowercase());

            let conn = eds::session_bus()?;
            let sources = list_task_sources(&conn)?;

            let mut tasks = Vec::new();
            let mut notes = Vec::new();
            for src in &sources {
                if let Some(f) = &filter {
                    if !src.name.to_lowercase().contains(f.as_str()) {
                        continue;
                    }
                }
                match eds::open(&conn, FACTORY_METHOD, &src.uid)
                    .and_then(|path| eds::get_object_list(&conn, &path, MATCH_ALL))
                {
                    Ok(blobs) => {
                        for blob in blobs {
                            for mut task in parse_tasks(&blob) {
                                if !include_completed && task.completed() {
                                    continue;
                                }
                                task.list = src.name.clone();
                                tasks.push(task);
                            }
                        }
                    }
                    Err(err) => notes.push(format!("- {} ({}): {}", src.name, src.uid, err)),
                }
            }
            tasks.sort_by(|a, b| a.sort_key.cmp(&b.sort_key));

            let mut out = String::new();
            if tasks.is_empty() {
                out.push_str(if include_completed {
                    "No tasks found."
                } else {
                    "No open tasks found."
                });
            } else {
                for task in &tasks {
                    out.push_str(&render_task(task));
                }
            }
            if !notes.is_empty() {
                out.push_str("\nNotes (task lists that could not be read):\n");
                out.push_str(&notes.join("\n"));
            }
            truncate(&mut out, MAX_OUTPUT_BYTES);
            Ok(out)
        },
    )
}

fn render_task(task: &Task) -> String {
    let check = if task.completed() { "[x]" } else { "[ ]" };
    let title = if task.summary.is_empty() {
        "(no title)"
    } else {
        &task.summary
    };
    let mut s = format!("• {check} {title}  [{}]\n", task.list);
    if let Some(due) = &task.due_raw {
        s.push_str(&format!("    Due: {}\n", eds::format_dt(due, task.due_all_day)));
    }
    // Surface a non-default status (IN-PROCESS, CANCELLED); NEEDS-ACTION and
    // COMPLETED are already conveyed by the checkbox.
    if let Some(status) = &task.status {
        let upper = status.to_ascii_uppercase();
        if upper != "NEEDS-ACTION" && upper != "COMPLETED" {
            s.push_str(&format!("    Status: {status}\n"));
        }
    }
    if let Some(desc) = task.description.as_deref().filter(|d| !d.is_empty()) {
        s.push_str(&format!("    Notes: {}\n", desc.replace('\n', " ")));
    }
    if let Some(uid) = &task.uid {
        s.push_str(&format!("    uid: {uid}\n"));
    }
    s
}

fn create_task_tool() -> Tool {
    Tool::new(
        "tasks_create_task",
        "Create a new to-do task. Writes to the user's real task list.",
        json!({
            "type": "object",
            "properties": {
                "summary": {"type": "string", "description": "Task title."},
                "due": {"type": "string", "description": "Optional due date/time, ISO 8601. A date only (YYYY-MM-DD) is an all-day due date. Naive times are UTC."},
                "description": {"type": "string", "description": "Optional notes."},
                "list": {"type": "string", "description": "Optional target task list (name substring or uid). Defaults to the local task list."}
            },
            "required": ["summary"],
            "additionalProperties": false
        }),
        |args| {
            let summary = args["summary"]
                .as_str()
                .ok_or("`summary` must be a string")?;

            let conn = eds::session_bus()?;
            let sources = list_task_sources(&conn)?;
            let target = eds::resolve_target(&sources, args["list"].as_str(), DEFAULT_LIST_UID)?;
            let path = eds::open(&conn, FACTORY_METHOD, &target.uid)?;

            let uid = eds::new_uid();
            let ics = build_vtodo(&uid, summary, args["due"].as_str(), args["description"].as_str())?;
            eds::create_object(&conn, &path, &ics)?;
            Ok(format!("Created task \"{summary}\" in {} (uid: {uid}).", target.name))
        },
    )
}

fn modify_task_tool() -> Tool {
    Tool::new(
        "tasks_modify_task",
        "Modify a task, identified by its uid (from tasks_list_tasks). Only the \
         fields you provide are changed. Set `completed` to true to mark a task \
         done or false to reopen it. Writes to the user's real task list.",
        json!({
            "type": "object",
            "properties": {
                "uid": {"type": "string", "description": "uid of the task to modify."},
                "summary": {"type": "string"},
                "due": {"type": "string", "description": "New due date/time, ISO 8601 (date only = all-day). Use an empty string to clear the due date."},
                "description": {"type": "string"},
                "completed": {"type": "boolean", "description": "Mark the task completed (true) or reopen it (false)."}
            },
            "required": ["uid"],
            "additionalProperties": false
        }),
        |args| {
            let uid = args["uid"].as_str().ok_or("`uid` must be a string")?;
            let summary = args["summary"].as_str();
            let due = args["due"].as_str();
            let description = args["description"].as_str();
            let completed = args["completed"].as_bool();
            if summary.is_none() && due.is_none() && description.is_none() && completed.is_none() {
                return Err("nothing to modify: provide at least one field to change".into());
            }

            let conn = eds::session_bus()?;
            let sources = list_task_sources(&conn)?;
            let (path, src) = eds::find_object(&conn, &sources, uid, FACTORY_METHOD)?;
            let existing = eds::get_object(&conn, &path, uid)?;
            let mut lines = eds::unfold(&existing);

            if let Some(s) = summary {
                eds::set_property(&mut lines, "SUMMARY", Some(format!("SUMMARY:{}", eds::escape(s))));
            }
            if let Some(d) = due {
                let line = if d.is_empty() {
                    None
                } else {
                    Some(eds::dt_property("DUE", d)?)
                };
                eds::set_property(&mut lines, "DUE", line);
            }
            if let Some(d) = description {
                eds::set_property(
                    &mut lines,
                    "DESCRIPTION",
                    Some(format!("DESCRIPTION:{}", eds::escape(d))),
                );
            }
            if let Some(done) = completed {
                set_completion(&mut lines, done);
            }

            let new_ics = lines.join("\r\n");
            eds::modify_object(&conn, &path, &new_ics)?;
            Ok(format!("Modified task {uid} in {}.", src.name))
        },
    )
}

fn delete_task_tool() -> Tool {
    Tool::new(
        "tasks_delete_task",
        "Delete a task, identified by its uid (from tasks_list_tasks). \
         Permanently removes it from the user's real task list.",
        json!({
            "type": "object",
            "properties": {
                "uid": {"type": "string", "description": "uid of the task to delete."}
            },
            "required": ["uid"],
            "additionalProperties": false
        }),
        |args| {
            let uid = args["uid"].as_str().ok_or("`uid` must be a string")?;
            let conn = eds::session_bus()?;
            let sources = list_task_sources(&conn)?;
            let (path, src) = eds::find_object(&conn, &sources, uid, FACTORY_METHOD)?;
            eds::remove_object(&conn, &path, uid)?;
            Ok(format!("Deleted task {uid} from {}.", src.name))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_tasks_reads_fields() {
        let ics = "BEGIN:VTODO\r\nUID:t1\r\nSUMMARY:Buy mi\r\n lk\r\nDUE;VALUE=DATE:20260620\r\nSTATUS:NEEDS-ACTION\r\nEND:VTODO\r\n";
        let tasks = parse_tasks(ics);
        assert_eq!(tasks.len(), 1);
        let t = &tasks[0];
        assert_eq!(t.summary, "Buy milk");
        assert_eq!(t.uid.as_deref(), Some("t1"));
        assert!(t.due_all_day);
        assert!(!t.completed());
    }

    #[test]
    fn completed_detection() {
        let ics = "BEGIN:VTODO\r\nUID:t\r\nSUMMARY:Done\r\nSTATUS:COMPLETED\r\nEND:VTODO\r\n";
        assert!(parse_tasks(ics)[0].completed());
    }

    #[test]
    fn build_vtodo_defaults_to_open() {
        let v = build_vtodo("u1", "Pay; rent", Some("2026-06-25"), Some("via bank")).unwrap();
        assert!(v.contains("SUMMARY:Pay\\; rent\r\n")); // escaped semicolon
        assert!(v.contains("STATUS:NEEDS-ACTION\r\n"));
        assert!(v.contains("DUE;VALUE=DATE:20260625\r\n"));
        assert!(v.contains("DESCRIPTION:via bank\r\n"));
    }

    #[test]
    fn set_completion_toggles_trio() {
        let mut lines = eds::unfold("BEGIN:VTODO\r\nUID:u\r\nSTATUS:NEEDS-ACTION\r\nPERCENT-COMPLETE:0\r\nEND:VTODO\r\n");
        set_completion(&mut lines, true);
        let done = lines.join("\r\n");
        assert!(done.contains("STATUS:COMPLETED"));
        assert!(done.contains("PERCENT-COMPLETE:100"));
        assert!(done.contains("COMPLETED:"));

        set_completion(&mut lines, false);
        let open = lines.join("\r\n");
        assert!(open.contains("STATUS:NEEDS-ACTION"));
        assert!(open.contains("PERCENT-COMPLETE:0"));
        assert!(!open.contains("\nCOMPLETED:") && !open.starts_with("COMPLETED:"));
    }

    // Live end-to-end CRUD against the running Evolution Data Server. Ignored by
    // default (touches the real task list / needs the session bus); run with
    // `cargo test --release -- --ignored live_tasks`.
    #[test]
    #[ignore]
    fn live_tasks_roundtrip() {
        let conn = eds::session_bus().expect("session bus");
        let sources = list_task_sources(&conn).expect("list sources");
        assert!(
            sources.iter().any(|s| s.uid == DEFAULT_LIST_UID),
            "expected a {DEFAULT_LIST_UID} source, got {:?}",
            sources.iter().map(|s| &s.uid).collect::<Vec<_>>()
        );
        let path = eds::open(&conn, FACTORY_METHOD, DEFAULT_LIST_UID).expect("open");

        let uid = eds::new_uid();
        let ics = build_vtodo(&uid, "Beckon task test", Some("2026-06-25"), None).expect("build");
        eds::create_object(&conn, &path, &ics).expect("create");

        let fetched = eds::get_object(&conn, &path, &uid).expect("get after create");
        assert!(fetched.contains("SUMMARY:Beckon task test"));

        let mut lines = eds::unfold(&fetched);
        set_completion(&mut lines, true);
        eds::modify_object(&conn, &path, &lines.join("\r\n")).expect("complete");
        let after = eds::get_object(&conn, &path, &uid).expect("get after complete");
        assert!(parse_tasks(&after)[0].completed());

        eds::remove_object(&conn, &path, &uid).expect("remove");
        assert!(eds::get_object(&conn, &path, &uid).is_err(), "task should be gone");
    }
}
