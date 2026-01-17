/*
 * SPDX-License-Identifier: GPL-3.0-or-later
 * SPDX-FileCopyrightText: 2026 breitburg
 */

public class ElementaryIntelligence.Database : Object {
    private Sqlite.Database db;
    private static Database? instance;

    public static Database get_default () {
        if (instance == null) {
            instance = new Database ();
        }
        return instance;
    }

    private Database () {
        var data_dir = Path.build_filename (
            Environment.get_user_data_dir (),
            "com.github.breitburg.elementary-intelligence"
        );

        DirUtils.create_with_parents (data_dir, 0755);

        var db_path = Path.build_filename (data_dir, "chats.db");
        var result = Sqlite.Database.open (db_path, out db);

        if (result != Sqlite.OK) {
            critical ("Failed to open database: %s", db.errmsg ());
            return;
        }

        initialize_tables ();
    }

    private void initialize_tables () {
        string sql = """
            CREATE TABLE IF NOT EXISTS chats (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                title TEXT NOT NULL DEFAULT 'New Chat',
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                chat_id INTEGER NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                FOREIGN KEY (chat_id) REFERENCES chats(id) ON DELETE CASCADE
            );
        """;

        string errmsg;
        var result = db.exec (sql, null, out errmsg);

        if (result != Sqlite.OK) {
            critical ("Failed to create tables: %s", errmsg);
        }
    }

    public Chat create_chat (string title = "New Chat") {
        var now = new DateTime.now_local ();
        var timestamp = now.to_unix ();

        string sql = "INSERT INTO chats (title, created_at, updated_at) VALUES (?, ?, ?)";
        Sqlite.Statement stmt;
        db.prepare_v2 (sql, -1, out stmt);
        stmt.bind_text (1, title);
        stmt.bind_int64 (2, timestamp);
        stmt.bind_int64 (3, timestamp);
        stmt.step ();

        var chat = new Chat ();
        chat.id = db.last_insert_rowid ();
        chat.title = title;
        chat.created_at = now;
        chat.updated_at = now;

        return chat;
    }

    public Gee.ArrayList<Chat> get_all_chats () {
        var chats = new Gee.ArrayList<Chat> ();

        string sql = "SELECT id, title, created_at, updated_at FROM chats ORDER BY updated_at DESC";
        Sqlite.Statement stmt;
        db.prepare_v2 (sql, -1, out stmt);

        while (stmt.step () == Sqlite.ROW) {
            var chat = new Chat.with_id (
                stmt.column_int64 (0),
                stmt.column_text (1),
                new DateTime.from_unix_local (stmt.column_int64 (2)),
                new DateTime.from_unix_local (stmt.column_int64 (3))
            );
            chats.add (chat);
        }

        return chats;
    }

    public Chat? get_chat (int64 id) {
        string sql = "SELECT id, title, created_at, updated_at FROM chats WHERE id = ?";
        Sqlite.Statement stmt;
        db.prepare_v2 (sql, -1, out stmt);
        stmt.bind_int64 (1, id);

        if (stmt.step () == Sqlite.ROW) {
            return new Chat.with_id (
                stmt.column_int64 (0),
                stmt.column_text (1),
                new DateTime.from_unix_local (stmt.column_int64 (2)),
                new DateTime.from_unix_local (stmt.column_int64 (3))
            );
        }

        return null;
    }

    public void update_chat_title (int64 chat_id, string title) {
        var now = new DateTime.now_local ().to_unix ();

        string sql = "UPDATE chats SET title = ?, updated_at = ? WHERE id = ?";
        Sqlite.Statement stmt;
        db.prepare_v2 (sql, -1, out stmt);
        stmt.bind_text (1, title);
        stmt.bind_int64 (2, now);
        stmt.bind_int64 (3, chat_id);
        stmt.step ();
    }

    public void update_chat_timestamp (int64 chat_id) {
        var now = new DateTime.now_local ().to_unix ();

        string sql = "UPDATE chats SET updated_at = ? WHERE id = ?";
        Sqlite.Statement stmt;
        db.prepare_v2 (sql, -1, out stmt);
        stmt.bind_int64 (1, now);
        stmt.bind_int64 (2, chat_id);
        stmt.step ();
    }

    public void delete_chat (int64 chat_id) {
        string sql = "DELETE FROM messages WHERE chat_id = ?";
        Sqlite.Statement stmt;
        db.prepare_v2 (sql, -1, out stmt);
        stmt.bind_int64 (1, chat_id);
        stmt.step ();

        sql = "DELETE FROM chats WHERE id = ?";
        db.prepare_v2 (sql, -1, out stmt);
        stmt.bind_int64 (1, chat_id);
        stmt.step ();
    }

    public Message add_message (int64 chat_id, MessageRole role, string content) {
        var now = new DateTime.now_local ();
        var timestamp = now.to_unix ();

        string sql = "INSERT INTO messages (chat_id, role, content, created_at) VALUES (?, ?, ?, ?)";
        Sqlite.Statement stmt;
        db.prepare_v2 (sql, -1, out stmt);
        stmt.bind_int64 (1, chat_id);
        stmt.bind_text (2, role.to_string ());
        stmt.bind_text (3, content);
        stmt.bind_int64 (4, timestamp);
        stmt.step ();

        update_chat_timestamp (chat_id);

        var message = new Message.with_id (
            db.last_insert_rowid (),
            chat_id,
            role,
            content,
            now
        );

        return message;
    }

    public Gee.ArrayList<Message> get_messages_for_chat (int64 chat_id) {
        var messages = new Gee.ArrayList<Message> ();

        string sql = "SELECT id, chat_id, role, content, created_at FROM messages WHERE chat_id = ? ORDER BY created_at ASC";
        Sqlite.Statement stmt;
        db.prepare_v2 (sql, -1, out stmt);
        stmt.bind_int64 (1, chat_id);

        while (stmt.step () == Sqlite.ROW) {
            var message = new Message.with_id (
                stmt.column_int64 (0),
                stmt.column_int64 (1),
                MessageRole.from_string (stmt.column_text (2)),
                stmt.column_text (3),
                new DateTime.from_unix_local (stmt.column_int64 (4))
            );
            messages.add (message);
        }

        return messages;
    }
}
