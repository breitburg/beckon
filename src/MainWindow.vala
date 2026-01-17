/*
 * SPDX-License-Identifier: GPL-3.0-or-later
 * SPDX-FileCopyrightText: 2026 breitburg
 */

public class ElementaryIntelligence.MainWindow : Adw.ApplicationWindow {
    private GLib.Settings settings;
    private ChatSidebar sidebar;
    private Gtk.Stack content_stack;
    private WelcomeView welcome_view;
    private Gee.HashMap<int64?, ChatView> chat_views;
    private Gtk.Label content_title_label;
    private Adw.OverlaySplitView split_view;
    private int64 current_chat_id = -1;

    public MainWindow (Gtk.Application application) {
        Object (
            application: application,
            title: "Elementary Intelligence"
        );
    }

    construct {
        settings = new GLib.Settings ("com.github.breitburg.elementary-intelligence");
        chat_views = new Gee.HashMap<int64?, ChatView> ();

        default_width = settings.get_int ("window-width");
        default_height = settings.get_int ("window-height");

        var sidebar_header = new Adw.HeaderBar () {
            show_end_title_buttons = false
        };
        sidebar_header.add_css_class ("flat");
        sidebar_header.title_widget = new Gtk.Label ("Chats");

        var new_chat_button = new Gtk.Button.from_icon_name ("list-add-symbolic") {
            tooltip_text = "New Chat"
        };
        new_chat_button.clicked.connect (create_new_chat);
        sidebar_header.pack_start (new_chat_button);

        sidebar = new ChatSidebar ();
        sidebar.chat_selected.connect (on_chat_selected);
        sidebar.chat_deleted.connect (on_chat_delete_requested);

        var sidebar_box = new Gtk.Box (Gtk.Orientation.VERTICAL, 0);
        sidebar_box.add_css_class ("sidebar-box");
        sidebar_box.append (sidebar_header);
        sidebar_box.append (sidebar);

        content_title_label = new Gtk.Label (null) {
            use_markup = true,
            label = "<b>Elementary Intelligence</b>"
        };

        var content_header = new Adw.HeaderBar () {
            show_start_title_buttons = false,
            title_widget = content_title_label
        };
        content_header.add_css_class ("flat");

        var toggle_button = new Gtk.ToggleButton () {
            icon_name = "sidebar-show-symbolic",
            tooltip_text = "Toggle Sidebar",
            active = true
        };
        split_view = new Adw.OverlaySplitView () {
            sidebar_width_fraction = 0.3,
            min_sidebar_width = 200,
            max_sidebar_width = 400
        };
        toggle_button.bind_property ("active", split_view, "show-sidebar", BindingFlags.BIDIRECTIONAL | BindingFlags.SYNC_CREATE);
        content_header.pack_start (toggle_button);

        var settings_button = new Gtk.Button.from_icon_name ("open-menu-symbolic") {
            tooltip_text = "Settings"
        };
        settings_button.clicked.connect (show_settings);
        content_header.pack_end (settings_button);

        welcome_view = new WelcomeView ();
        welcome_view.new_chat_requested.connect (create_new_chat);

        content_stack = new Gtk.Stack () {
            transition_type = Gtk.StackTransitionType.CROSSFADE,
            vexpand = true,
            hexpand = true
        };
        content_stack.add_named (welcome_view, "welcome");

        var content_box = new Gtk.Box (Gtk.Orientation.VERTICAL, 0) {
            hexpand = true
        };
        content_box.append (content_header);
        content_box.append (content_stack);

        split_view.sidebar = sidebar_box;
        split_view.content = content_box;

        var breakpoint = new Adw.Breakpoint (Adw.BreakpointCondition.parse ("max-width: 600sp"));
        breakpoint.add_setter (split_view, "collapsed", true);
        add_breakpoint (breakpoint);

        content = split_view;

        restore_last_chat ();

        close_request.connect (() => {
            save_window_state ();
            return false;
        });
    }

    private void save_window_state () {
        settings.set_int ("window-width", get_width ());
        settings.set_int ("window-height", get_height ());
        settings.set_int ("last-chat-id", (int) current_chat_id);
    }

    private void restore_last_chat () {
        var last_chat_id = settings.get_int ("last-chat-id");
        if (last_chat_id > 0) {
            var db = Database.get_default ();
            var chat = db.get_chat (last_chat_id);
            if (chat != null) {
                sidebar.select_chat (last_chat_id);
                on_chat_selected (chat);
                return;
            }
        }

        content_stack.visible_child_name = "welcome";
    }

    private void create_new_chat () {
        var db = Database.get_default ();
        var chats = db.get_all_chats ();

        // Check if latest chat is empty, if so just switch to it
        if (chats.size > 0) {
            var latest_chat = chats.get (0);
            var messages = db.get_messages_for_chat (latest_chat.id);
            if (messages.size == 0) {
                sidebar.select_chat (latest_chat.id);
                on_chat_selected (latest_chat);
                return;
            }
        }

        var chat = db.create_chat ();

        sidebar.load_chats ();
        sidebar.select_chat (chat.id);
        on_chat_selected (chat);
    }

    private void on_chat_selected (Chat chat) {
        current_chat_id = chat.id;

        var stack_name = "chat-%lld".printf (chat.id);

        if (!chat_views.has_key (chat.id)) {
            var chat_view = new ChatView (chat);
            chat_view.chat_updated.connect (() => {
                sidebar.load_chats ();
                sidebar.select_chat (chat.id);
                content_title_label.label = "<b>" + GLib.Markup.escape_text (chat_view.chat.title) + "</b>";
            });
            chat_views.set (chat.id, chat_view);
            content_stack.add_named (chat_view, stack_name);
        }

        content_title_label.label = "<b>" + GLib.Markup.escape_text (chat.title) + "</b>";
        content_stack.visible_child_name = stack_name;

        if (split_view.collapsed) {
            split_view.show_sidebar = false;
        }
    }

    private void on_chat_delete_requested (Chat chat) {
        var dialog = new Granite.MessageDialog.with_image_from_icon_name (
            "Delete Chat?",
            "This will permanently delete the chat and all its messages.",
            "dialog-warning",
            Gtk.ButtonsType.NONE
        );
        dialog.transient_for = this;
        dialog.modal = true;

        dialog.add_button ("Cancel", Gtk.ResponseType.CANCEL);
        var delete_button = dialog.add_button ("Delete", Gtk.ResponseType.ACCEPT);
        delete_button.add_css_class (Granite.CssClass.DESTRUCTIVE);

        dialog.response.connect ((response_id) => {
            if (response_id == Gtk.ResponseType.ACCEPT) {
                delete_chat (chat);
            }
            dialog.destroy ();
        });

        dialog.present ();
    }

    private void delete_chat (Chat chat) {
        var db = Database.get_default ();
        db.delete_chat (chat.id);

        var stack_name = "chat-%lld".printf (chat.id);
        var chat_view = content_stack.get_child_by_name (stack_name);
        if (chat_view != null) {
            content_stack.remove (chat_view);
        }
        chat_views.unset (chat.id);

        if (current_chat_id == chat.id) {
            current_chat_id = -1;
            content_title_label.label = "<b>Elementary Intelligence</b>";
            content_stack.visible_child_name = "welcome";
        }

        sidebar.load_chats ();
    }

    private void show_settings () {
        var dialog = new SettingsDialog (this);
        dialog.present ();
    }
}
