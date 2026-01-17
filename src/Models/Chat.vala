/*
 * SPDX-License-Identifier: GPL-3.0-or-later
 * SPDX-FileCopyrightText: 2026 breitburg
 */

public enum ElementaryIntelligence.DateGroup {
    TODAY,
    YESTERDAY,
    THIS_WEEK,
    EARLIER;

    public string to_string () {
        switch (this) {
            case TODAY:
                return "Today";
            case YESTERDAY:
                return "Yesterday";
            case THIS_WEEK:
                return "This Week";
            case EARLIER:
                return "Earlier";
            default:
                return "Earlier";
        }
    }
}

public class ElementaryIntelligence.Chat : Object {
    public int64 id { get; set; default = -1; }
    public string title { get; set; default = "New Chat"; }
    public DateTime created_at { get; set; }
    public DateTime updated_at { get; set; }

    public Chat () {
        var now = new DateTime.now_local ();
        created_at = now;
        updated_at = now;
    }

    public Chat.with_id (int64 id, string title, DateTime created_at, DateTime updated_at) {
        this.id = id;
        this.title = title;
        this.created_at = created_at;
        this.updated_at = updated_at;
    }

    public DateGroup get_date_group () {
        var now = new DateTime.now_local ();
        var today_start = new DateTime.local (now.get_year (), now.get_month (), now.get_day_of_month (), 0, 0, 0);
        var yesterday_start = today_start.add_days (-1);
        var week_start = today_start.add_days (-7);

        if (updated_at.compare (today_start) >= 0) {
            return DateGroup.TODAY;
        } else if (updated_at.compare (yesterday_start) >= 0) {
            return DateGroup.YESTERDAY;
        } else if (updated_at.compare (week_start) >= 0) {
            return DateGroup.THIS_WEEK;
        } else {
            return DateGroup.EARLIER;
        }
    }
}
