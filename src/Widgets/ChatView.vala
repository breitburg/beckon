/*
 * SPDX-License-Identifier: GPL-3.0-or-later
 * SPDX-FileCopyrightText: 2026 breitburg
 */

public class ElementaryIntelligence.ChatView : Gtk.Box {
    private Chat _chat;
    private Gtk.Box messages_box;
    private Gtk.Entry input_entry;
    private Gtk.Button send_button;
    private Gtk.ScrolledWindow scrolled_window;
    private MessageRow? streaming_row;
    private string streaming_content;
    private Granite.Toast toast;
    private bool is_streaming = false;

    public signal void chat_updated ();

    public Chat chat {
        get { return _chat; }
    }

    public ChatView (Chat chat) {
        Object (
            orientation: Gtk.Orientation.VERTICAL,
            spacing: 0
        );

        _chat = chat;
        load_messages ();
    }

    construct {
        messages_box = new Gtk.Box (Gtk.Orientation.VERTICAL, 0) {
            vexpand = true
        };

        scrolled_window = new Gtk.ScrolledWindow () {
            hscrollbar_policy = Gtk.PolicyType.NEVER,
            vscrollbar_policy = Gtk.PolicyType.AUTOMATIC,
            vexpand = true,
            child = messages_box
        };

        input_entry = new Gtk.Entry () {
            placeholder_text = "Type a message...",
            hexpand = true
        };
        input_entry.add_css_class ("flat");
        input_entry.activate.connect (on_send_clicked);

        send_button = new Gtk.Button.with_label ("Send");
        send_button.add_css_class ("flat");
        send_button.add_css_class (Granite.CssClass.SUGGESTED);
        send_button.clicked.connect (on_send_clicked);

        var input_box = new Gtk.Box (Gtk.Orientation.HORIZONTAL, 0) {
            margin_top = 6,
            margin_bottom = 6,
            margin_start = 12,
            margin_end = 12
        };
        input_box.append (input_entry);
        input_box.append (send_button);

        var input_frame = new Gtk.Box (Gtk.Orientation.VERTICAL, 0);
        input_frame.add_css_class ("message-input-box");
        input_frame.append (input_box);

        toast = new Granite.Toast ("");

        var content_box = new Gtk.Box (Gtk.Orientation.VERTICAL, 0);
        content_box.append (scrolled_window);
        content_box.append (input_frame);

        var overlay = new Gtk.Overlay () {
            child = content_box
        };
        overlay.add_overlay (toast);

        append (overlay);
        vexpand = true;
        hexpand = true;

        var ai_service = AIService.get_default ();
        ai_service.stream_started.connect (on_stream_started);
        ai_service.stream_delta.connect (on_stream_delta);
        ai_service.stream_finished.connect (on_stream_finished);
        ai_service.stream_error.connect (on_stream_error);
    }

    private void load_messages () {
        var db = Database.get_default ();
        var messages = db.get_messages_for_chat (_chat.id);

        foreach (var message in messages) {
            if (message.role != MessageRole.SYSTEM) {
                var row = new MessageRow (message.role, message.content);
                messages_box.append (row);
            }
        }

        scroll_to_bottom ();
    }

    private void on_send_clicked () {
        var text = input_entry.text.strip ();
        if (text == "") {
            return;
        }

        input_entry.text = "";
        set_input_sensitive (false);

        var db = Database.get_default ();
        db.add_message (_chat.id, MessageRole.USER, text);

        var user_row = new MessageRow (MessageRole.USER, text);
        messages_box.append (user_row);

        if (_chat.title == "New Chat") {
            var title = text.length > 30 ? text.substring (0, 30) + "..." : text;
            db.update_chat_title (_chat.id, title);
            _chat.title = title;
        }

        chat_updated ();
        scroll_to_bottom ();

        is_streaming = true;
        send_to_ai.begin ();
    }

    private async void send_to_ai () {
        var db = Database.get_default ();
        var messages = db.get_messages_for_chat (_chat.id);
        var ai_service = AIService.get_default ();

        yield ai_service.send_message_streaming (messages);
    }

    private void on_stream_started () {
        if (!is_streaming) return;

        streaming_content = "";
        streaming_row = new MessageRow (MessageRole.ASSISTANT, "");
        messages_box.append (streaming_row);
        scroll_to_bottom ();
    }

    private void on_stream_delta (string content) {
        if (!is_streaming) return;

        streaming_content += content;
        if (streaming_row != null) {
            streaming_row.content = streaming_content;
        }
        scroll_to_bottom ();
    }

    private void on_stream_finished () {
        if (!is_streaming) return;

        if (streaming_content != "") {
            var db = Database.get_default ();
            db.add_message (_chat.id, MessageRole.ASSISTANT, streaming_content);
            chat_updated ();
        }

        streaming_row = null;
        streaming_content = "";
        is_streaming = false;
        set_input_sensitive (true);
        input_entry.grab_focus ();
    }

    private void on_stream_error (string error_message) {
        if (!is_streaming) return;

        if (streaming_row != null) {
            messages_box.remove (streaming_row);
            streaming_row = null;
        }

        streaming_content = "";
        is_streaming = false;
        set_input_sensitive (true);
        input_entry.grab_focus ();

        toast.title = error_message;
        toast.send_notification ();
    }

    private void set_input_sensitive (bool sensitive) {
        input_entry.sensitive = sensitive;
        send_button.sensitive = sensitive;
    }

    private void scroll_to_bottom () {
        Idle.add (() => {
            var adj = scrolled_window.vadjustment;
            adj.value = adj.upper - adj.page_size;
            return false;
        });
    }
}
