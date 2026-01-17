/*
 * SPDX-License-Identifier: GPL-3.0-or-later
 * SPDX-FileCopyrightText: 2026 breitburg
 */

public class ElementaryIntelligence.ChatSidebar : Gtk.Box {
    private Gtk.ListBox list_box;
    private Gee.HashMap<int64?, ChatRow> chat_rows;

    public signal void chat_selected (Chat chat);
    public signal void chat_deleted (Chat chat);

    public ChatSidebar () {
        Object (
            orientation: Gtk.Orientation.VERTICAL,
            spacing: 0
        );
    }

    construct {
        chat_rows = new Gee.HashMap<int64?, ChatRow> ();

        list_box = new Gtk.ListBox () {
            selection_mode = Gtk.SelectionMode.SINGLE,
            vexpand = true
        };
        list_box.add_css_class ("navigation-sidebar");
        list_box.set_header_func (update_header);
        list_box.row_selected.connect (on_row_selected);

        var scrolled = new Gtk.ScrolledWindow () {
            hscrollbar_policy = Gtk.PolicyType.NEVER,
            vexpand = true,
            child = list_box
        };

        append (scrolled);

        load_chats ();
    }

    private void update_header (Gtk.ListBoxRow row, Gtk.ListBoxRow? before) {
        var chat_row = row as ChatRow;
        if (chat_row == null) {
            return;
        }

        var current_group = chat_row.chat.get_date_group ();

        if (before == null) {
            var header = new Granite.HeaderLabel (current_group.to_string ());
            row.set_header (header);
            return;
        }

        var before_row = before as ChatRow;
        if (before_row == null) {
            return;
        }

        var before_group = before_row.chat.get_date_group ();

        if (current_group != before_group) {
            var header = new Granite.HeaderLabel (current_group.to_string ());
            row.set_header (header);
        } else {
            row.set_header (null);
        }
    }

    private void on_row_selected (Gtk.ListBoxRow? row) {
        if (row == null) {
            return;
        }

        var chat_row = row as ChatRow;
        if (chat_row != null) {
            chat_selected (chat_row.chat);
        }
    }

    public void load_chats () {
        var child = list_box.get_first_child ();
        while (child != null) {
            var next = child.get_next_sibling ();
            list_box.remove (child);
            child = next;
        }

        chat_rows.clear ();

        var db = Database.get_default ();
        var chats = db.get_all_chats ();

        foreach (var chat in chats) {
            add_chat_row (chat);
        }
    }

    private void add_chat_row (Chat chat) {
        var row = new ChatRow (chat);
        row.delete_requested.connect (() => {
            chat_deleted (chat);
        });
        chat_rows.set (chat.id, row);
        list_box.append (row);
    }

    public void select_chat (int64 chat_id) {
        if (chat_rows.has_key (chat_id)) {
            var row = chat_rows.get (chat_id);
            list_box.select_row (row);
        }
    }

    public void add_and_select_chat (Chat chat) {
        load_chats ();
        select_chat (chat.id);
    }

    public void refresh () {
        list_box.invalidate_headers ();
    }
}

public class ElementaryIntelligence.ChatRow : Gtk.ListBoxRow {
    public Chat chat { get; private set; }

    public signal void delete_requested ();

    public ChatRow (Chat chat) {
        this.chat = chat;

        var label = new Gtk.Label (chat.title) {
            xalign = 0,
            ellipsize = Pango.EllipsizeMode.END,
            margin_top = 6,
            margin_bottom = 6,
            margin_start = 6,
            margin_end = 6
        };

        child = label;

        var gesture = new Gtk.GestureClick () {
            button = Gdk.BUTTON_SECONDARY
        };
        gesture.pressed.connect (on_right_click);
        add_controller (gesture);
    }

    private void on_right_click (int n_press, double x, double y) {
        var menu = new Gtk.PopoverMenu.from_model (create_menu ());
        menu.set_parent (this);
        menu.popup ();
    }

    private Menu create_menu () {
        var menu = new Menu ();
        var delete_item = new MenuItem ("Delete", "chat.delete");
        menu.append_item (delete_item);

        var action_group = new SimpleActionGroup ();
        var delete_action = new SimpleAction ("delete", null);
        delete_action.activate.connect (() => {
            delete_requested ();
        });
        action_group.add_action (delete_action);
        insert_action_group ("chat", action_group);

        return menu;
    }

    public void update_title (string title) {
        chat.title = title;
        var label = child as Gtk.Label;
        if (label != null) {
            label.label = title;
        }
    }
}
