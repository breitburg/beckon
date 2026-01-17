/*
 * SPDX-License-Identifier: GPL-3.0-or-later
 * SPDX-FileCopyrightText: 2026 breitburg
 */

public class ElementaryIntelligence.MessageRow : Gtk.Box {
    private Gtk.Label content_label;
    private MessageRole _role;
    private string _raw_content = "";

    public string content {
        get { return _raw_content; }
        set {
            _raw_content = value;
            update_markup ();
        }
    }

    public MessageRole role {
        get { return _role; }
    }

    public MessageRow (MessageRole role, string content = "") {
        Object (
            orientation: Gtk.Orientation.VERTICAL,
            spacing: 4,
            margin_top: 6,
            margin_bottom: 6,
            margin_start: 12,
            margin_end: 12
        );

        _role = role;

        var role_label = new Gtk.Label (role == MessageRole.USER ? "You" : "Assistant") {
            xalign = 0,
            halign = role == MessageRole.USER ? Gtk.Align.END : Gtk.Align.START
        };
        role_label.add_css_class (Granite.CssClass.DIM);
        role_label.add_css_class (Granite.CssClass.SMALL);

        content_label = new Gtk.Label ("") {
            wrap = true,
            wrap_mode = Pango.WrapMode.WORD_CHAR,
            xalign = 0,
            selectable = true,
            use_markup = true
        };
        content_label.set_cursor_from_name ("default");

        var content_frame = new Gtk.Frame (null) {
            halign = role == MessageRole.USER ? Gtk.Align.END : Gtk.Align.START,
            margin_start = role == MessageRole.USER ? 48 : 0,
            margin_end = role == MessageRole.USER ? 0 : 48
        };
        content_frame.add_css_class (Granite.CssClass.CARD);
        content_frame.add_css_class ("message-bubble");
        content_frame.child = content_label;

        content_label.margin_top = 8;
        content_label.margin_bottom = 8;
        content_label.margin_start = 12;
        content_label.margin_end = 12;

        if (role == MessageRole.USER) {
            content_frame.add_css_class ("user-message");
            content_label.add_css_class ("user-message-content");
            role_label.add_css_class ("user-message-content");
        }

        append (role_label);
        append (content_frame);

        // Set initial content
        this.content = content;
    }

    private void update_markup () {
        if (_role == MessageRole.USER) {
            // Don't render markdown for user messages
            content_label.label = MarkdownRenderer.to_pango (_raw_content);
        } else {
            content_label.label = MarkdownRenderer.to_pango (_raw_content);
        }
    }

    public void append_content (string text) {
        _raw_content += text;
        update_markup ();
    }
}
