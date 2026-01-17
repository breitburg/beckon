/*
 * SPDX-License-Identifier: GPL-3.0-or-later
 * SPDX-FileCopyrightText: 2026 breitburg
 */

public class ElementaryIntelligence.WelcomeView : Granite.Placeholder {
    public signal void new_chat_requested ();

    public WelcomeView () {
        Object (
            title: "Elementary Intelligence",
            description: "Start a conversation with AI",
            icon: new ThemedIcon ("dialog-information")
        );
    }

    construct {
        var new_chat_button = append_button (
            new ThemedIcon ("list-add"),
            "New Chat",
            "Start a new conversation"
        );
        new_chat_button.clicked.connect (() => {
            new_chat_requested ();
        });
    }
}
