// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 breitburg

//! Calendar toolset backed by Evolution Data Server (EDS) over D-Bus.
//!
//! elementary's Calendar app is a frontend on EDS, so the same data is reachable
//! through the session-bus calendar factory. The D-Bus plumbing, source registry,
//! and iCalendar text handling live in [`crate::eds`]; this module adds only the
//! VEVENT-specific parse/build/render and the four tools the model calls.
//!
//! Dates are handled as UTC with exact integer math (no `chrono`): query bounds
//! are formatted as EDS `make-time` UTC strings, and stored event times are
//! displayed as their wall-clock value with a `UTC`/`all day` marker rather than
//! being converted between zones.

use serde_json::json;

use crate::datetime::{epoch_to_date, epoch_to_make_time, now_epoch, parse_iso};
use crate::eds::{self, Source};
use crate::tools::{truncate, Tool, MAX_OUTPUT_BYTES};

/// EDS source INI group identifying a calendar (vs. a task or memo list).
const SOURCE_GROUP: &str = "[Calendar]";

/// CalendarFactory method that opens a calendar by source UID.
const FACTORY_METHOD: &str = "OpenCalendar";

/// UID of the built-in writable local calendar ("Personal"); the default
/// create/modify target when the model names no calendar.
const DEFAULT_CAL_UID: &str = "system-calendar";

/// Builds the calendar tools exposed to the model. One settings toggle
/// ("calendar") enables this whole set.
pub fn tools() -> Vec<Tool> {
    vec![
        list_events_tool(),
        create_event_tool(),
        modify_event_tool(),
        delete_event_tool(),
    ]
}

fn list_calendar_sources(conn: &gtk4::gio::DBusConnection) -> Result<Vec<Source>, String> {
    eds::list_sources(conn, SOURCE_GROUP)
}

// ---------------------------------------------------------------------------
// iCalendar (RFC 5545) — VEVENT parse and build
// ---------------------------------------------------------------------------

struct Event {
    summary: String,
    start_raw: Option<String>,
    end_raw: Option<String>,
    all_day: bool,
    location: Option<String>,
    uid: Option<String>,
    calendar: String,
    sort_key: String,
}

/// Parse one VEVENT's inner lines into an [`Event`] (calendar set by caller).
fn parse_vevent(lines: &[String]) -> Event {
    let mut ev = Event {
        summary: String::new(),
        start_raw: None,
        end_raw: None,
        all_day: false,
        location: None,
        uid: None,
        calendar: String::new(),
        sort_key: "0".to_string(),
    };
    for line in lines {
        let Some((name, params, value)) = eds::split_property(line) else {
            continue;
        };
        match name.as_str() {
            "SUMMARY" => ev.summary = eds::unescape(&value),
            "DTSTART" => {
                ev.all_day = eds::is_date_only(&params, &value);
                ev.start_raw = Some(value);
            }
            "DTEND" => ev.end_raw = Some(value),
            "LOCATION" => ev.location = Some(eds::unescape(&value)),
            "UID" => ev.uid = Some(value),
            _ => {}
        }
    }
    ev.sort_key = eds::sort_key(&ev.start_raw);
    ev
}

/// Parse every VEVENT in a reply element (a bare VEVENT or a VCALENDAR holding
/// one or more).
fn parse_events(blob: &str) -> Vec<Event> {
    eds::components(blob, "VEVENT")
        .iter()
        .map(|lines| parse_vevent(lines))
        .collect()
}

/// Build a bare VEVENT for creation.
fn build_vevent(
    uid: &str,
    start: &str,
    all_day: bool,
    end: Option<&str>,
    summary: &str,
    location: Option<&str>,
    description: Option<&str>,
) -> Result<String, String> {
    let (start_secs, _) = parse_iso(start)?;
    let mut v = String::from("BEGIN:VEVENT\r\n");
    v.push_str(&format!("UID:{uid}\r\n"));
    v.push_str(&format!("DTSTAMP:{}\r\n", epoch_to_make_time(now_epoch())));
    if all_day {
        v.push_str(&format!("DTSTART;VALUE=DATE:{}\r\n", epoch_to_date(start_secs)));
        let end_secs = match end {
            Some(e) => parse_iso(e)?.0,
            None => start_secs + 86_400,
        };
        v.push_str(&format!("DTEND;VALUE=DATE:{}\r\n", epoch_to_date(end_secs)));
    } else {
        v.push_str(&format!("DTSTART:{}\r\n", epoch_to_make_time(start_secs)));
        let end_secs = match end {
            Some(e) => parse_iso(e)?.0,
            None => start_secs + 3600,
        };
        v.push_str(&format!("DTEND:{}\r\n", epoch_to_make_time(end_secs)));
    }
    v.push_str(&format!("SUMMARY:{}\r\n", eds::escape(summary)));
    if let Some(loc) = location.filter(|l| !l.is_empty()) {
        v.push_str(&format!("LOCATION:{}\r\n", eds::escape(loc)));
    }
    if let Some(desc) = description.filter(|d| !d.is_empty()) {
        v.push_str(&format!("DESCRIPTION:{}\r\n", eds::escape(desc)));
    }
    v.push_str("END:VEVENT\r\n");
    Ok(v)
}

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

fn list_events_tool() -> Tool {
    Tool::new(
        "calendar_list_events",
        "List calendar events in a time range across all the user's calendars. \
         Returns each event's title, time, location, calendar, and uid (use the \
         uid to modify or delete an event).",
        json!({
            "type": "object",
            "properties": {
                "start": {"type": "string", "description": "Start of range, ISO 8601 (YYYY-MM-DD or YYYY-MM-DDTHH:MM:SSZ). Naive times are UTC. Defaults to now."},
                "end": {"type": "string", "description": "End of range, ISO 8601. Defaults to 7 days after start."},
                "calendar": {"type": "string", "description": "Optional: only this calendar (case-insensitive name substring)."}
            },
            "additionalProperties": false
        }),
        |args| {
            let now = now_epoch();
            let (start_secs, _) = match args["start"].as_str() {
                Some(s) => parse_iso(s)?,
                None => (now, false),
            };
            let (end_secs, _) = match args["end"].as_str() {
                Some(s) => parse_iso(s)?,
                None => (start_secs + 7 * 86_400, false),
            };
            let filter = args["calendar"].as_str().map(|s| s.to_lowercase());

            let conn = eds::session_bus()?;
            let sources = list_calendar_sources(&conn)?;
            let query = format!(
                "(occur-in-time-range? (make-time \"{}\") (make-time \"{}\"))",
                epoch_to_make_time(start_secs),
                epoch_to_make_time(end_secs)
            );

            let mut events = Vec::new();
            let mut notes = Vec::new();
            for src in &sources {
                if let Some(f) = &filter {
                    if !src.name.to_lowercase().contains(f.as_str()) {
                        continue;
                    }
                }
                match eds::open(&conn, FACTORY_METHOD, &src.uid)
                    .and_then(|path| eds::get_object_list(&conn, &path, &query))
                {
                    Ok(blobs) => {
                        for blob in blobs {
                            for mut ev in parse_events(&blob) {
                                ev.calendar = src.name.clone();
                                events.push(ev);
                            }
                        }
                    }
                    Err(err) => notes.push(format!("- {} ({}): {}", src.name, src.uid, err)),
                }
            }
            events.sort_by(|a, b| a.sort_key.cmp(&b.sort_key));

            let mut out = String::new();
            if events.is_empty() {
                out.push_str(&format!(
                    "No events found between {} and {}.",
                    eds::format_dt(&epoch_to_make_time(start_secs), false),
                    eds::format_dt(&epoch_to_make_time(end_secs), false)
                ));
            } else {
                for ev in &events {
                    out.push_str(&render_event(ev));
                }
            }
            if !notes.is_empty() {
                out.push_str("\nNotes (calendars that could not be read):\n");
                out.push_str(&notes.join("\n"));
            }
            truncate(&mut out, MAX_OUTPUT_BYTES);
            Ok(out)
        },
    )
}

fn render_event(ev: &Event) -> String {
    let title = if ev.summary.is_empty() {
        "(no title)"
    } else {
        &ev.summary
    };
    let mut s = format!("• {title}  [{}]\n", ev.calendar);
    let when = match (&ev.start_raw, &ev.end_raw) {
        (Some(start), Some(end)) => format!(
            "    {} – {}\n",
            eds::format_dt(start, ev.all_day),
            eds::format_dt(end, ev.all_day)
        ),
        (Some(start), None) => format!("    {}\n", eds::format_dt(start, ev.all_day)),
        _ => String::new(),
    };
    s.push_str(&when);
    if let Some(loc) = ev.location.as_deref().filter(|l| !l.is_empty()) {
        s.push_str(&format!("    Location: {loc}\n"));
    }
    if let Some(uid) = &ev.uid {
        s.push_str(&format!("    uid: {uid}\n"));
    }
    s
}

fn create_event_tool() -> Tool {
    Tool::new(
        "calendar_create_event",
        "Create a new calendar event. Writes to the user's real calendar.",
        json!({
            "type": "object",
            "properties": {
                "summary": {"type": "string", "description": "Event title."},
                "start": {"type": "string", "description": "Start, ISO 8601. A date only (YYYY-MM-DD) creates an all-day event; include a time for a timed event. Naive times are UTC."},
                "end": {"type": "string", "description": "End, ISO 8601. Defaults to 1 hour after a timed start, or the next day for all-day."},
                "location": {"type": "string"},
                "description": {"type": "string"},
                "calendar": {"type": "string", "description": "Optional target calendar (name substring or uid). Defaults to the local Personal calendar."}
            },
            "required": ["summary", "start"],
            "additionalProperties": false
        }),
        |args| {
            let summary = args["summary"]
                .as_str()
                .ok_or("`summary` must be a string")?;
            let start = args["start"].as_str().ok_or("`start` must be a string")?;
            let (_, has_time) = parse_iso(start)?;
            let all_day = !has_time;

            let conn = eds::session_bus()?;
            let sources = list_calendar_sources(&conn)?;
            let target = eds::resolve_target(&sources, args["calendar"].as_str(), DEFAULT_CAL_UID)?;
            let path = eds::open(&conn, FACTORY_METHOD, &target.uid)?;

            let uid = eds::new_uid();
            let ics = build_vevent(
                &uid,
                start,
                all_day,
                args["end"].as_str(),
                summary,
                args["location"].as_str(),
                args["description"].as_str(),
            )?;
            eds::create_object(&conn, &path, &ics)?;
            Ok(format!(
                "Created \"{summary}\" in {} (uid: {uid}).",
                target.name
            ))
        },
    )
}

fn modify_event_tool() -> Tool {
    Tool::new(
        "calendar_modify_event",
        "Modify an existing event, identified by its uid (from calendar_list_events). \
         Only the fields you provide are changed. Writes to the user's real calendar.",
        json!({
            "type": "object",
            "properties": {
                "uid": {"type": "string", "description": "uid of the event to modify."},
                "summary": {"type": "string"},
                "start": {"type": "string", "description": "New start, ISO 8601 (date only = all-day)."},
                "end": {"type": "string", "description": "New end, ISO 8601."},
                "location": {"type": "string"},
                "description": {"type": "string"}
            },
            "required": ["uid"],
            "additionalProperties": false
        }),
        |args| {
            let uid = args["uid"].as_str().ok_or("`uid` must be a string")?;
            let summary = args["summary"].as_str();
            let start = args["start"].as_str();
            let end = args["end"].as_str();
            let location = args["location"].as_str();
            let description = args["description"].as_str();
            if summary.is_none()
                && start.is_none()
                && end.is_none()
                && location.is_none()
                && description.is_none()
            {
                return Err("nothing to modify: provide at least one field to change".into());
            }

            let conn = eds::session_bus()?;
            let sources = list_calendar_sources(&conn)?;
            let (path, src) = eds::find_object(&conn, &sources, uid, FACTORY_METHOD)?;
            let existing = eds::get_object(&conn, &path, uid)?;
            let mut lines = eds::unfold(&existing);

            if let Some(s) = summary {
                eds::set_property(&mut lines, "SUMMARY", Some(format!("SUMMARY:{}", eds::escape(s))));
            }
            if let Some(s) = start {
                eds::set_property(&mut lines, "DTSTART", Some(eds::dt_property("DTSTART", s)?));
            }
            if let Some(e) = end {
                eds::set_property(&mut lines, "DTEND", Some(eds::dt_property("DTEND", e)?));
            }
            if let Some(l) = location {
                eds::set_property(&mut lines, "LOCATION", Some(format!("LOCATION:{}", eds::escape(l))));
            }
            if let Some(d) = description {
                eds::set_property(
                    &mut lines,
                    "DESCRIPTION",
                    Some(format!("DESCRIPTION:{}", eds::escape(d))),
                );
            }

            let new_ics = lines.join("\r\n");
            eds::modify_object(&conn, &path, &new_ics)?;
            Ok(format!("Modified event {uid} in {}.", src.name))
        },
    )
}

fn delete_event_tool() -> Tool {
    Tool::new(
        "calendar_delete_event",
        "Delete an event, identified by its uid (from calendar_list_events). \
         Permanently removes it from the user's real calendar.",
        json!({
            "type": "object",
            "properties": {
                "uid": {"type": "string", "description": "uid of the event to delete."}
            },
            "required": ["uid"],
            "additionalProperties": false
        }),
        |args| {
            let uid = args["uid"].as_str().ok_or("`uid` must be a string")?;
            let conn = eds::session_bus()?;
            let sources = list_calendar_sources(&conn)?;
            let (path, src) = eds::find_object(&conn, &sources, uid, FACTORY_METHOD)?;
            eds::remove_object(&conn, &path, uid)?;
            Ok(format!("Deleted event {uid} from {}.", src.name))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_events_handles_folding() {
        let ics = "BEGIN:VEVENT\r\nUID:abc\r\nSUMMARY:Team sy\r\n nc\r\nDTSTART:20260615T090000Z\r\nDTEND:20260615T093000Z\r\nLOCATION:Room 4\r\nEND:VEVENT\r\n";
        let events = parse_events(ics);
        assert_eq!(events.len(), 1);
        let ev = &events[0];
        assert_eq!(ev.summary, "Team sync");
        assert_eq!(ev.uid.as_deref(), Some("abc"));
        assert_eq!(ev.location.as_deref(), Some("Room 4"));
        assert!(!ev.all_day);
        assert_eq!(ev.sort_key, "20260615090000");
    }

    #[test]
    fn all_day_detection() {
        let ics = "BEGIN:VEVENT\r\nUID:x\r\nDTSTART;VALUE=DATE:20260620\r\nSUMMARY:Holiday\r\nEND:VEVENT\r\n";
        let ev = &parse_events(ics)[0];
        assert!(ev.all_day);
    }

    #[test]
    fn build_vevent_timed_and_all_day() {
        let timed = build_vevent("u1", "2026-06-20T15:00:00", false, None, "Meet; greet", None, None).unwrap();
        assert!(timed.contains("DTSTART:20260620T150000Z\r\n"));
        assert!(timed.contains("DTEND:20260620T160000Z\r\n")); // +1h default
        assert!(timed.contains("SUMMARY:Meet\\; greet\r\n")); // escaped semicolon

        let allday = build_vevent("u2", "2026-06-20", true, None, "Trip", None, None).unwrap();
        assert!(allday.contains("DTSTART;VALUE=DATE:20260620\r\n"));
        assert!(allday.contains("DTEND;VALUE=DATE:20260621\r\n")); // next day default
    }

    // Live end-to-end CRUD against the running Evolution Data Server. Ignored by
    // default (touches the real calendar / needs the session bus); run with
    // `cargo test --release -- --ignored live_crud`.
    #[test]
    #[ignore]
    fn live_crud_roundtrip() {
        let conn = eds::session_bus().expect("session bus");
        let sources = list_calendar_sources(&conn).expect("list sources");
        assert!(
            sources.iter().any(|s| s.uid == DEFAULT_CAL_UID),
            "expected a {DEFAULT_CAL_UID} source, got {:?}",
            sources.iter().map(|s| &s.uid).collect::<Vec<_>>()
        );
        let path = eds::open(&conn, FACTORY_METHOD, DEFAULT_CAL_UID).expect("open");

        let uid = eds::new_uid();
        let ics = build_vevent(&uid, "2026-06-20T15:00:00", false, None, "Beckon CRUD test", Some("Room 1"), None)
            .expect("build");
        eds::create_object(&conn, &path, &ics).expect("create");

        let fetched = eds::get_object(&conn, &path, &uid).expect("get after create");
        assert!(fetched.contains("SUMMARY:Beckon CRUD test"));

        let mut lines = eds::unfold(&fetched);
        eds::set_property(&mut lines, "SUMMARY", Some("SUMMARY:Beckon CRUD modified".to_string()));
        eds::modify_object(&conn, &path, &lines.join("\r\n")).expect("modify");
        let after = eds::get_object(&conn, &path, &uid).expect("get after modify");
        assert!(after.contains("SUMMARY:Beckon CRUD modified"));

        eds::remove_object(&conn, &path, &uid).expect("remove");
        assert!(eds::get_object(&conn, &path, &uid).is_err(), "event should be gone");
    }
}
