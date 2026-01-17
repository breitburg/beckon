/*
 * SPDX-License-Identifier: GPL-3.0-or-later
 * SPDX-FileCopyrightText: 2026 breitburg
 */

public class ElementaryIntelligence.AIService : Object {
    private Soup.Session session;
    private GLib.Settings settings;
    private Cancellable? current_cancellable;

    public signal void stream_started ();
    public signal void stream_delta (string content);
    public signal void stream_finished ();
    public signal void stream_error (string error_message);

    private static AIService? instance;

    public static AIService get_default () {
        if (instance == null) {
            instance = new AIService ();
        }
        return instance;
    }

    private AIService () {
        session = new Soup.Session ();
        settings = new GLib.Settings ("com.github.breitburg.elementary-intelligence");
    }

    public void cancel_request () {
        if (current_cancellable != null) {
            current_cancellable.cancel ();
            current_cancellable = null;
        }
    }

    public async void send_message_streaming (Gee.ArrayList<Message> messages) {
        cancel_request ();
        current_cancellable = new Cancellable ();

        var base_url = settings.get_string ("api-base-url");
        var api_key = settings.get_string ("api-key");
        var model = settings.get_string ("model-name");

        if (api_key == "") {
            stream_error ("API key not configured. Please set your API key in Settings.");
            return;
        }

        var url = base_url + "/chat/completions";

        var builder = new Json.Builder ();
        builder.begin_object ();
        builder.set_member_name ("model");
        builder.add_string_value (model);
        builder.set_member_name ("stream");
        builder.add_boolean_value (true);
        builder.set_member_name ("messages");
        builder.begin_array ();

        foreach (var message in messages) {
            builder.begin_object ();
            builder.set_member_name ("role");
            builder.add_string_value (message.role.to_string ());
            builder.set_member_name ("content");
            builder.add_string_value (message.content);
            builder.end_object ();
        }

        builder.end_array ();
        builder.end_object ();

        var generator = new Json.Generator ();
        generator.set_root (builder.get_root ());
        var json_body = generator.to_data (null);

        var msg = new Soup.Message ("POST", url);
        msg.request_headers.append ("Authorization", "Bearer " + api_key);
        msg.request_headers.append ("Content-Type", "application/json");
        msg.set_request_body_from_bytes ("application/json", new Bytes (json_body.data));

        stream_started ();

        try {
            var input_stream = yield session.send_async (msg, Priority.DEFAULT, current_cancellable);

            if (msg.status_code != 200) {
                var error_data = new StringBuilder ();
                var buffer = new uint8[4096];
                while (true) {
                    var bytes_read = yield input_stream.read_async (buffer, Priority.DEFAULT, current_cancellable);
                    if (bytes_read == 0) break;
                    error_data.append ((string) buffer[0:bytes_read]);
                }
                stream_error ("API Error (%u): %s".printf (msg.status_code, error_data.str));
                return;
            }

            var data_stream = new DataInputStream (input_stream);
            var buffer = new StringBuilder ();

            while (true) {
                if (current_cancellable.is_cancelled ()) {
                    break;
                }

                string? line = null;
                try {
                    line = yield data_stream.read_line_async (Priority.DEFAULT, current_cancellable);
                } catch (IOError e) {
                    if (e is IOError.CANCELLED) {
                        break;
                    }
                    throw e;
                }

                if (line == null) {
                    break;
                }

                if (line.has_prefix ("data: ")) {
                    var json_str = line.substring (6).strip ();

                    if (json_str == "[DONE]") {
                        break;
                    }

                    try {
                        var parser = new Json.Parser ();
                        parser.load_from_data (json_str);
                        var root = parser.get_root ().get_object ();
                        var choices = root.get_array_member ("choices");

                        if (choices.get_length () > 0) {
                            var choice = choices.get_object_element (0);
                            var delta = choice.get_object_member ("delta");

                            if (delta.has_member ("content")) {
                                var content = delta.get_string_member ("content");
                                if (content != null && content != "") {
                                    Idle.add (() => {
                                        stream_delta (content);
                                        return false;
                                    });
                                }
                            }
                        }
                    } catch (Error e) {
                        // Skip malformed JSON chunks
                        continue;
                    }
                }
            }

            stream_finished ();

        } catch (Error e) {
            if (!(e is IOError.CANCELLED)) {
                stream_error ("Connection error: " + e.message);
            }
        }

        current_cancellable = null;
    }
}
