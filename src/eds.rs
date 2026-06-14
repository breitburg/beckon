// SPDX-License-Identifier: GPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 breitburg

//! Shared Evolution Data Server (EDS) plumbing for the calendar and tasks
//! toolsets.
//!
//! elementary's Calendar and Tasks apps are both frontends on EDS, which serves
//! calendars, task lists, and memo lists from one session-bus factory
//! (`org.gnome.evolution.dataserver.Calendar8`). The flow is identical for each:
//! `CalendarFactory.Open{Calendar,TaskList}(uid)` returns a per-list object path,
//! `Calendar.Open()` activates it, then `GetObjectList` / `GetObject` read and
//! `CreateObjects` / `ModifyObjects` / `RemoveObjects` write. The available lists
//! come from the `Sources5` registry's ObjectManager, distinguished by the INI
//! group in their source data (`[Calendar]` vs `[Task List]`).
//!
//! This module owns that bus traffic plus the minimal iCalendar (RFC 5545)
//! text handling both component types share (VEVENT and VTODO use the same
//! property syntax). The component-specific parse/build/render lives in the
//! `calendar` and `tasks` modules.
//!
//! All D-Bus uses `gtk4::gio` (no extra dependency). The callers run on the API
//! worker thread (blocking, `Send + Sync`); gio's synchronous calls are safe
//! there — GDBus drives them on its own worker.

use std::sync::atomic::{AtomicU64, Ordering};

use gtk4::gio;
use gtk4::glib::Variant;
use gtk4::prelude::*;

use crate::datetime::{epoch_to_date, epoch_to_make_time, now_epoch, parse_iso};

const DEST: &str = "org.gnome.evolution.dataserver.Calendar8";
const FACTORY_PATH: &str = "/org/gnome/evolution/dataserver/CalendarFactory";
const FACTORY_IFACE: &str = "org.gnome.evolution.dataserver.CalendarFactory";
const CLIENT_IFACE: &str = "org.gnome.evolution.dataserver.Calendar";
const SOURCES_DEST: &str = "org.gnome.evolution.dataserver.Sources5";
const SOURCES_PATH: &str = "/org/gnome/evolution/dataserver/SourceManager";
const OBJECT_MANAGER_IFACE: &str = "org.freedesktop.DBus.ObjectManager";

/// D-Bus timeout. Kept modest so a stalled network list can't hang the worker
/// thread for long (the agent loop has no per-tool timeout of its own).
const DBUS_TIMEOUT_MS: i32 = 5000;

/// `ECalObjModType` GEnum nick for modifying/removing a single (non-recurring)
/// instance. The local backend rejects `all` (it advertises `no-thisandprior`),
/// and we always operate on a whole object by UID, so `this` is correct.
pub(crate) const MOD_TYPE_THIS: &str = "this";

// ---------------------------------------------------------------------------
// D-Bus plumbing
// ---------------------------------------------------------------------------

pub(crate) fn session_bus() -> Result<gio::DBusConnection, String> {
    gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)
        .map_err(|err| format!("session bus: {err}"))
}

/// One blocking D-Bus call against the EDS factory destination. `reply_type` is
/// left unchecked (`None`): we read the reply variant structurally, so EDS
/// adding fields can't break us.
pub(crate) fn call(
    conn: &gio::DBusConnection,
    path: &str,
    iface: &str,
    method: &str,
    params: Option<&Variant>,
) -> Result<Variant, String> {
    conn.call_sync(
        Some(DEST),
        path,
        iface,
        method,
        params,
        None,
        gio::DBusCallFlags::NONE,
        DBUS_TIMEOUT_MS,
        gio::Cancellable::NONE,
    )
    .map_err(|err| format!("{method}: {err}"))
}

/// `factory_method(uid)` → object path, then `Open()` to activate it.
/// `factory_method` is `OpenCalendar` for calendars or `OpenTaskList` for tasks.
pub(crate) fn open(
    conn: &gio::DBusConnection,
    factory_method: &str,
    uid: &str,
) -> Result<String, String> {
    let reply = call(
        conn,
        FACTORY_PATH,
        FACTORY_IFACE,
        factory_method,
        Some(&(uid,).to_variant()),
    )?;
    let path = reply
        .child_value(0)
        .str()
        .map(|s| s.to_string())
        .ok_or_else(|| format!("{factory_method}: unexpected reply"))?;
    call(conn, &path, CLIENT_IFACE, "Open", None)?;
    Ok(path)
}

/// Objects matching an EDS S-expression query, as raw iCalendar strings.
pub(crate) fn get_object_list(
    conn: &gio::DBusConnection,
    path: &str,
    query: &str,
) -> Result<Vec<String>, String> {
    let reply = call(
        conn,
        path,
        CLIENT_IFACE,
        "GetObjectList",
        Some(&(query,).to_variant()),
    )?;
    Ok(string_array(&reply.child_value(0)))
}

/// One object's iCalendar by UID (empty `rid` = the master, non-recurring).
/// Returns `Err` if the list doesn't hold it.
pub(crate) fn get_object(
    conn: &gio::DBusConnection,
    path: &str,
    uid: &str,
) -> Result<String, String> {
    let reply = call(
        conn,
        path,
        CLIENT_IFACE,
        "GetObject",
        Some(&(uid, "").to_variant()),
    )?;
    reply
        .child_value(0)
        .str()
        .map(|s| s.to_string())
        .ok_or_else(|| "GetObject: unexpected reply".to_string())
}

/// Create one component (a bare VEVENT/VTODO — EDS rejects a VCALENDAR wrapper).
pub(crate) fn create_object(
    conn: &gio::DBusConnection,
    path: &str,
    ics: &str,
) -> Result<(), String> {
    call(
        conn,
        path,
        CLIENT_IFACE,
        "CreateObjects",
        Some(&(vec![ics.to_string()], 0u32).to_variant()),
    )?;
    Ok(())
}

pub(crate) fn modify_object(
    conn: &gio::DBusConnection,
    path: &str,
    ics: &str,
) -> Result<(), String> {
    call(
        conn,
        path,
        CLIENT_IFACE,
        "ModifyObjects",
        Some(&(vec![ics.to_string()], MOD_TYPE_THIS, 0u32).to_variant()),
    )?;
    Ok(())
}

pub(crate) fn remove_object(
    conn: &gio::DBusConnection,
    path: &str,
    uid: &str,
) -> Result<(), String> {
    let targets = vec![(uid.to_string(), String::new())];
    call(
        conn,
        path,
        CLIENT_IFACE,
        "RemoveObjects",
        Some(&(targets, MOD_TYPE_THIS, 0u32).to_variant()),
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Source registry (the available calendars / task lists)
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub(crate) struct Source {
    pub uid: String,
    pub name: String,
}

/// All sources from the `Sources5` ObjectManager whose data declares `group`
/// (`[Calendar]` for calendars, `[Task List]` for task lists).
pub(crate) fn list_sources(
    conn: &gio::DBusConnection,
    group: &str,
) -> Result<Vec<Source>, String> {
    let reply = conn
        .call_sync(
            Some(SOURCES_DEST),
            SOURCES_PATH,
            OBJECT_MANAGER_IFACE,
            "GetManagedObjects",
            None,
            None,
            gio::DBusCallFlags::NONE,
            DBUS_TIMEOUT_MS,
            gio::Cancellable::NONE,
        )
        .map_err(|err| format!("GetManagedObjects: {err}"))?;

    // Reply: (a{o a{s a{s v}}}) — managed objects, each a path → interfaces map.
    let objects = reply.child_value(0);
    let mut sources = Vec::new();
    for i in 0..objects.n_children() {
        let entry = objects.child_value(i); // {o, a{s a{s v}}}
        let ifaces = entry.child_value(1);
        let Some(data) = prop_string(&ifaces, "Data") else {
            continue;
        };
        if !data.contains(group) {
            continue;
        }
        let Some(uid) = prop_string(&ifaces, "UID") else {
            continue;
        };
        let name = ini_value(&data, "DisplayName").unwrap_or_else(|| uid.clone());
        sources.push(Source { uid, name });
    }
    Ok(sources)
}

/// Open every source and return the (path, source) of the first holding `uid`.
/// `factory_method` selects the list type (see [`open`]).
pub(crate) fn find_object(
    conn: &gio::DBusConnection,
    sources: &[Source],
    uid: &str,
    factory_method: &str,
) -> Result<(String, Source), String> {
    for src in sources {
        if let Ok(path) = open(conn, factory_method, &src.uid) {
            if let Ok(ics) = get_object(conn, &path, uid) {
                if !ics.is_empty() {
                    return Ok((path, src.clone()));
                }
            }
        }
    }
    Err(format!("no object with uid \"{uid}\" found"))
}

/// Resolve the target list for a write: by name/uid substring if the model
/// named one, else the default local list (falling back to the first).
pub(crate) fn resolve_target(
    sources: &[Source],
    named: Option<&str>,
    default_uid: &str,
) -> Result<Source, String> {
    match named {
        Some(name) => {
            let needle = name.to_lowercase();
            sources
                .iter()
                .find(|s| s.uid == name || s.name.to_lowercase().contains(&needle))
                .cloned()
                .ok_or_else(|| format!("no list matching \"{name}\""))
        }
        None => sources
            .iter()
            .find(|s| s.uid == default_uid)
            .or_else(|| sources.first())
            .cloned()
            .ok_or_else(|| "no lists available".to_string()),
    }
}

/// Find the string value of property `key` anywhere in an interfaces map
/// (`a{s a{s v}}`), unwrapping the `v` wrapper.
fn prop_string(ifaces: &Variant, key: &str) -> Option<String> {
    for i in 0..ifaces.n_children() {
        let iface_entry = ifaces.child_value(i); // {s, a{s v}}
        let props = iface_entry.child_value(1); // a{s v}
        for j in 0..props.n_children() {
            let prop = props.child_value(j); // {s, v}
            if prop.child_value(0).str() == Some(key) {
                return prop
                    .child_value(1)
                    .as_variant()
                    .and_then(|v| v.str().map(|s| s.to_string()));
            }
        }
    }
    None
}

/// Value of `key=` from a `.source` INI string. Matches the bare key only, so
/// localized variants like `DisplayName[de]=` are ignored.
fn ini_value(ini: &str, key: &str) -> Option<String> {
    for line in ini.lines() {
        if let Some(rest) = line.strip_prefix(key) {
            if let Some(value) = rest.strip_prefix('=') {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// Extract a `Vec<String>` from a GVariant array (`as`).
fn string_array(arr: &Variant) -> Vec<String> {
    (0..arr.n_children())
        .filter_map(|i| arr.child_value(i).str().map(|s| s.to_string()))
        .collect()
}

// ---------------------------------------------------------------------------
// iCalendar (RFC 5545) text — shared by VEVENT and VTODO
// ---------------------------------------------------------------------------

/// Unfold logical lines: a line beginning with a space or tab continues the
/// previous one (RFC 5545 §3.1).
pub(crate) fn unfold(ics: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    for raw in ics.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if let Some(rest) = line.strip_prefix(' ').or_else(|| line.strip_prefix('\t')) {
            if let Some(last) = lines.last_mut() {
                last.push_str(rest);
                continue;
            }
        }
        lines.push(line.to_string());
    }
    lines
}

/// Inner lines of each `BEGIN:{kind} … END:{kind}` block in a reply blob (a bare
/// component or a VCALENDAR holding one or more).
pub(crate) fn components(blob: &str, kind: &str) -> Vec<Vec<String>> {
    let lines = unfold(blob);
    let begin = format!("BEGIN:{kind}");
    let end = format!("END:{kind}");
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if trimmed.eq_ignore_ascii_case(&begin) {
            start = Some(i + 1);
        } else if trimmed.eq_ignore_ascii_case(&end) {
            if let Some(s) = start.take() {
                out.push(lines[s..i].to_vec());
            }
        }
    }
    out
}

/// Split `NAME;PARAM=x:VALUE` into (uppercased name, params, value) at the first
/// unquoted colon.
pub(crate) fn split_property(line: &str) -> Option<(String, String, String)> {
    let mut in_quote = false;
    let mut colon = None;
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_quote = !in_quote,
            ':' if !in_quote => {
                colon = Some(i);
                break;
            }
            _ => {}
        }
    }
    let colon = colon?;
    let (head, value) = (&line[..colon], &line[colon + 1..]);
    let (name, params) = match head.find(';') {
        Some(s) => (&head[..s], &head[s + 1..]),
        None => (head, ""),
    };
    Some((name.to_ascii_uppercase(), params.to_string(), value.to_string()))
}

pub(crate) fn is_date_only(params: &str, value: &str) -> bool {
    params
        .split(';')
        .any(|p| p.eq_ignore_ascii_case("VALUE=DATE"))
        || (!value.contains('T') && value.len() == 8 && value.bytes().all(|b| b.is_ascii_digit()))
}

/// Zero-padded 14-digit key (YYYYMMDDHHMMSS) for chronological sorting; values
/// with no date sort last.
pub(crate) fn sort_key(raw: &Option<String>) -> String {
    let digits: String = raw
        .iter()
        .flat_map(|s| s.chars())
        .filter(|c| c.is_ascii_digit())
        .take(14)
        .collect();
    if digits.is_empty() {
        return "9".repeat(14);
    }
    format!("{digits:0<14}")
}

pub(crate) fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') | Some('N') => out.push('\n'),
                Some('\\') => out.push('\\'),
                Some(',') => out.push(','),
                Some(';') => out.push(';'),
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

pub(crate) fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            _ => out.push(c),
        }
    }
    out
}

/// Render an iCalendar date/time value for display. All-day → `YYYY-MM-DD`;
/// timed → `YYYY-MM-DD HH:MM` with a `UTC` suffix when the value carries `Z`.
pub(crate) fn format_dt(raw: &str, all_day: bool) -> String {
    let utc = raw.ends_with('Z');
    let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    if all_day || (!raw.contains('T') && digits.len() == 8) {
        if digits.len() >= 8 {
            return format!("{}-{}-{}", &digits[0..4], &digits[4..6], &digits[6..8]);
        }
        return raw.to_string();
    }
    if digits.len() >= 12 {
        let base = format!(
            "{}-{}-{} {}:{}",
            &digits[0..4],
            &digits[4..6],
            &digits[6..8],
            &digits[8..10],
            &digits[10..12]
        );
        return if utc { format!("{base} UTC") } else { base };
    }
    raw.to_string()
}

/// A date/time property line (e.g. `DTSTART`, `DUE`) from ISO input, emitting
/// `;VALUE=DATE` for date-only values and a UTC timestamp otherwise.
pub(crate) fn dt_property(name: &str, iso: &str) -> Result<String, String> {
    let (secs, has_time) = parse_iso(iso)?;
    Ok(if has_time {
        format!("{name}:{}", epoch_to_make_time(secs))
    } else {
        format!("{name};VALUE=DATE:{}", epoch_to_date(secs))
    })
}

/// Replace (or remove) all lines of property `name` in an unfolded component,
/// inserting `new_line` just before the component's closing `END:` line. The
/// terminator is the *last* `END:` line, so a nested block (e.g. a VEVENT's
/// VALARM) doesn't capture the insertion point.
pub(crate) fn set_property(lines: &mut Vec<String>, name: &str, new_line: Option<String>) {
    lines.retain(|l| {
        let prop = l
            .split([';', ':'])
            .next()
            .unwrap_or("")
            .to_ascii_uppercase();
        prop != name
    });
    if let Some(line) = new_line {
        // Insert at the terminator's index so the new line lands just before it.
        let pos = lines
            .iter()
            .rposition(|l| l.trim().to_ascii_uppercase().starts_with("END:"))
            .unwrap_or(lines.len());
        lines.insert(pos, line);
    }
}

/// Process-unique component UID.
pub(crate) fn new_uid() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("beckon-{}-{}-{n}@beckon", now_epoch(), std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_dt_variants() {
        assert_eq!(format_dt("20260613", true), "2026-06-13");
        assert_eq!(format_dt("20260613T143000Z", false), "2026-06-13 14:30 UTC");
        assert_eq!(format_dt("20260613T143000", false), "2026-06-13 14:30");
    }

    #[test]
    fn components_handles_folding() {
        // A folded DESCRIPTION (continuation line begins with a space). RFC 5545
        // unfolding removes the CRLF *and* the continuation's leading space.
        let ics = "BEGIN:VEVENT\r\nUID:abc\r\nSUMMARY:Team sy\r\n nc\r\nEND:VEVENT\r\n";
        let comps = components(ics, "VEVENT");
        assert_eq!(comps.len(), 1);
        assert!(comps[0].iter().any(|l| l == "SUMMARY:Team sync"));
        assert!(comps[0].iter().any(|l| l == "UID:abc"));
    }

    #[test]
    fn sort_key_orders_dated_before_undated() {
        let dated = sort_key(&Some("20260615T090000Z".to_string()));
        let undated = sort_key(&None);
        assert_eq!(dated, "20260615090000");
        assert!(dated < undated);
    }

    #[test]
    fn set_property_replaces_before_end() {
        let mut lines = unfold("BEGIN:VTODO\r\nUID:u\r\nSUMMARY:Old\r\nEND:VTODO\r\n");
        set_property(&mut lines, "SUMMARY", Some("SUMMARY:New".to_string()));
        let out = lines.join("\r\n");
        assert!(out.contains("SUMMARY:New"));
        assert!(!out.contains("SUMMARY:Old"));
        assert!(out.contains("UID:u")); // untouched
        assert!(out.find("SUMMARY:New").unwrap() < out.find("END:VTODO").unwrap());
    }

    #[test]
    fn set_property_inserts_before_outer_end_past_nested_block() {
        // A VEVENT carrying a VALARM: the new line must land before END:VEVENT,
        // not the nested END:VALARM.
        let mut lines = unfold(
            "BEGIN:VEVENT\r\nUID:u\r\nBEGIN:VALARM\r\nACTION:DISPLAY\r\nEND:VALARM\r\nEND:VEVENT\r\n",
        );
        set_property(&mut lines, "SUMMARY", Some("SUMMARY:New".to_string()));
        let out = lines.join("\r\n");
        assert!(out.find("SUMMARY:New").unwrap() > out.find("END:VALARM").unwrap());
        assert!(out.find("SUMMARY:New").unwrap() < out.find("END:VEVENT").unwrap());
    }
}
