/*
 * SPDX-License-Identifier: GPL-3.0-or-later
 * SPDX-FileCopyrightText: 2026 breitburg
 */

public enum ElementaryIntelligence.MessageRole {
    USER,
    ASSISTANT,
    SYSTEM;

    public string to_string () {
        switch (this) {
            case USER:
                return "user";
            case ASSISTANT:
                return "assistant";
            case SYSTEM:
                return "system";
            default:
                return "user";
        }
    }

    public static MessageRole from_string (string role) {
        switch (role.down ()) {
            case "user":
                return USER;
            case "assistant":
                return ASSISTANT;
            case "system":
                return SYSTEM;
            default:
                return USER;
        }
    }
}

public class ElementaryIntelligence.Message : Object {
    public int64 id { get; set; default = -1; }
    public int64 chat_id { get; set; }
    public MessageRole role { get; set; default = MessageRole.USER; }
    public string content { get; set; default = ""; }
    public DateTime created_at { get; set; }

    public Message (int64 chat_id, MessageRole role, string content) {
        this.chat_id = chat_id;
        this.role = role;
        this.content = content;
        this.created_at = new DateTime.now_local ();
    }

    public Message.with_id (int64 id, int64 chat_id, MessageRole role, string content, DateTime created_at) {
        this.id = id;
        this.chat_id = chat_id;
        this.role = role;
        this.content = content;
        this.created_at = created_at;
    }
}
