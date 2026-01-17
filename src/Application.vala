/*
 * SPDX-License-Identifier: GPL-3.0-or-later
 * SPDX-FileCopyrightText: 2026 breitburg
 */

public class ElementaryIntelligence.Application : Gtk.Application {
    public Application () {
        Object (
            application_id: "com.github.breitburg.elementary-intelligence",
            flags: ApplicationFlags.DEFAULT_FLAGS
        );
    }

    protected override void activate () {
        Granite.init ();

        var provider = new Gtk.CssProvider ();
        provider.load_from_string ("""
            .user-message {
                background-color: @accent_color;
            }

            .user-message-content {
                color: white;
            }

            .message-input-box {
                border-top: 1px solid alpha(@borders, 0.5);
            }

            .message-bubble {
                box-shadow: 0 1px 2px alpha(black, 0.08);
                border-radius: 12px;
            }

            .sidebar-box {
                border-radius: 0;
            }

            .sidebar-box .navigation-sidebar {
                border-radius: 0;
            }
        """);
        Gtk.StyleContext.add_provider_for_display (
            Gdk.Display.get_default (),
            provider,
            Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION
        );

        var main_window = new MainWindow (this);
        main_window.present ();
    }

    public static int main (string[] args) {
        return new Application ().run (args);
    }
}
