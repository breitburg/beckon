/*
 * SPDX-License-Identifier: GPL-3.0-or-later
 * SPDX-FileCopyrightText: 2026 breitburg
 */

public class ElementaryIntelligence.SettingsDialog : Granite.Dialog {
    private Gtk.Entry base_url_entry;
    private Gtk.PasswordEntry api_key_entry;
    private Gtk.Entry model_entry;
    private GLib.Settings settings;

    public SettingsDialog (Gtk.Window parent) {
        Object (
            transient_for: parent,
            modal: true,
            title: "Settings"
        );
    }

    construct {
        settings = new GLib.Settings ("com.github.breitburg.elementary-intelligence");

        var base_url_label = new Granite.HeaderLabel ("API Base URL");
        base_url_entry = new Gtk.Entry () {
            text = settings.get_string ("api-base-url"),
            hexpand = true,
            placeholder_text = "https://api.openai.com/v1"
        };

        var api_key_label = new Granite.HeaderLabel ("API Key");
        api_key_entry = new Gtk.PasswordEntry () {
            show_peek_icon = true,
            hexpand = true,
            placeholder_text = "sk-..."
        };
        api_key_entry.set_text (settings.get_string ("api-key"));

        var model_label = new Granite.HeaderLabel ("Model");
        model_entry = new Gtk.Entry () {
            text = settings.get_string ("model-name"),
            hexpand = true,
            placeholder_text = "gpt-5"
        };

        var content_area = get_content_area ();
        content_area.margin_top = 12;
        content_area.margin_bottom = 12;
        content_area.margin_start = 12;
        content_area.margin_end = 12;
        content_area.spacing = 6;

        content_area.append (base_url_label);
        content_area.append (base_url_entry);
        content_area.append (api_key_label);
        content_area.append (api_key_entry);
        content_area.append (model_label);
        content_area.append (model_entry);

        add_button ("Cancel", Gtk.ResponseType.CANCEL);
        var save_button = add_button ("Save", Gtk.ResponseType.ACCEPT);
        save_button.add_css_class (Granite.CssClass.SUGGESTED);

        response.connect (on_response);

        default_width = 400;
    }

    private void on_response (int response_id) {
        if (response_id == Gtk.ResponseType.ACCEPT) {
            settings.set_string ("api-base-url", base_url_entry.text.strip ());
            settings.set_string ("api-key", api_key_entry.get_text ().strip ());
            settings.set_string ("model-name", model_entry.text.strip ());
        }

        close ();
    }
}
