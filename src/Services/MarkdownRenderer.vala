/*
 * SPDX-License-Identifier: GPL-3.0-or-later
 * SPDX-FileCopyrightText: 2026 breitburg
 */

public class ElementaryIntelligence.MarkdownRenderer : Object {

    public static string to_pango (string markdown) {
        // Use cmark-gfm to convert markdown to HTML
        var html = CmarkGfm.markdown_to_html (markdown, markdown.length, CmarkGfm.OPT_DEFAULT);

        // Convert HTML to Pango markup
        return html_to_pango (html);
    }

    private static string html_to_pango (string html) {
        var result = html;

        // Remove doctype, html, body tags if present
        result = strip_wrapper_tags (result);

        // Convert HTML tags to Pango markup

        // Headings - make them bold and larger isn't directly supported,
        // so we use bold and add newlines
        try {
            var h1_regex = new Regex ("<h1[^>]*>(.*?)</h1>", RegexCompileFlags.DOTALL);
            result = h1_regex.replace (result, -1, 0, "\n<span size=\"x-large\"><b>\\1</b></span>\n");

            var h2_regex = new Regex ("<h2[^>]*>(.*?)</h2>", RegexCompileFlags.DOTALL);
            result = h2_regex.replace (result, -1, 0, "\n<span size=\"large\"><b>\\1</b></span>\n");

            var h3_regex = new Regex ("<h3[^>]*>(.*?)</h3>", RegexCompileFlags.DOTALL);
            result = h3_regex.replace (result, -1, 0, "\n<b>\\1</b>\n");

            var h4_regex = new Regex ("<h4[^>]*>(.*?)</h4>", RegexCompileFlags.DOTALL);
            result = h4_regex.replace (result, -1, 0, "\n<b>\\1</b>\n");

            var h5_regex = new Regex ("<h5[^>]*>(.*?)</h5>", RegexCompileFlags.DOTALL);
            result = h5_regex.replace (result, -1, 0, "\n<b>\\1</b>\n");

            var h6_regex = new Regex ("<h6[^>]*>(.*?)</h6>", RegexCompileFlags.DOTALL);
            result = h6_regex.replace (result, -1, 0, "\n<b>\\1</b>\n");
        } catch (RegexError e) {
            warning ("Regex error in headings: %s", e.message);
        }

        // Bold
        result = result.replace ("<strong>", "<b>");
        result = result.replace ("</strong>", "</b>");

        // Italic
        result = result.replace ("<em>", "<i>");
        result = result.replace ("</em>", "</i>");

        // Code (inline)
        result = result.replace ("<code>", "<tt>");
        result = result.replace ("</code>", "</tt>");

        // Strikethrough (del tag from GFM)
        result = result.replace ("<del>", "<s>");
        result = result.replace ("</del>", "</s>");

        // Links - Pango supports <a href="">
        // Already in correct format from cmark

        // Paragraphs - convert to newlines
        try {
            var p_regex = new Regex ("<p>(.*?)</p>", RegexCompileFlags.DOTALL);
            result = p_regex.replace (result, -1, 0, "\\1\n\n");
        } catch (RegexError e) {
            warning ("Regex error in paragraphs: %s", e.message);
        }

        // Line breaks
        result = result.replace ("<br>", "\n");
        result = result.replace ("<br/>", "\n");
        result = result.replace ("<br />", "\n");

        // Code blocks (pre tags)
        try {
            var pre_regex = new Regex ("<pre[^>]*><tt>(.*?)</tt></pre>", RegexCompileFlags.DOTALL);
            result = pre_regex.replace (result, -1, 0, "\n<tt>\\1</tt>\n");
        } catch (RegexError e) {
            warning ("Regex error in pre blocks: %s", e.message);
        }

        // Lists - convert to simple text with bullets/numbers
        try {
            // Unordered list items
            var ul_li_regex = new Regex ("<li>(.*?)</li>", RegexCompileFlags.DOTALL);
            result = ul_li_regex.replace (result, -1, 0, "• \\1\n");

            // Remove ul/ol tags
            var ul_regex = new Regex ("</?ul[^>]*>", RegexCompileFlags.DOTALL);
            result = ul_regex.replace (result, -1, 0, "\n");

            var ol_regex = new Regex ("</?ol[^>]*>", RegexCompileFlags.DOTALL);
            result = ol_regex.replace (result, -1, 0, "\n");
        } catch (RegexError e) {
            warning ("Regex error in lists: %s", e.message);
        }

        // Blockquotes
        try {
            var bq_regex = new Regex ("<blockquote[^>]*>(.*?)</blockquote>", RegexCompileFlags.DOTALL);
            result = bq_regex.replace (result, -1, 0, "<i>\\1</i>");
        } catch (RegexError e) {
            warning ("Regex error in blockquotes: %s", e.message);
        }

        // Horizontal rules
        result = result.replace ("<hr>", "\n―――――――――――\n");
        result = result.replace ("<hr/>", "\n―――――――――――\n");
        result = result.replace ("<hr />", "\n―――――――――――\n");

        // Remove any remaining HTML tags that Pango doesn't understand
        try {
            var tag_regex = new Regex ("<(?!/?(?:b|i|u|s|tt|big|small|sub|sup|span|a)[ >])[^>]+>", RegexCompileFlags.CASELESS);
            result = tag_regex.replace (result, -1, 0, "");
        } catch (RegexError e) {
            warning ("Regex error removing tags: %s", e.message);
        }

        // Clean up excessive newlines
        try {
            var newline_regex = new Regex ("\n{3,}");
            result = newline_regex.replace (result, -1, 0, "\n\n");
        } catch (RegexError e) {
            warning ("Regex error in newlines: %s", e.message);
        }

        return result.strip ();
    }

    private static string strip_wrapper_tags (string html) {
        var result = html;

        try {
            // Remove DOCTYPE
            var doctype_regex = new Regex ("<!DOCTYPE[^>]*>", RegexCompileFlags.CASELESS);
            result = doctype_regex.replace (result, -1, 0, "");

            // Remove html tags
            var html_regex = new Regex ("</?html[^>]*>", RegexCompileFlags.CASELESS);
            result = html_regex.replace (result, -1, 0, "");

            // Remove head section
            var head_regex = new Regex ("<head[^>]*>.*?</head>", RegexCompileFlags.CASELESS | RegexCompileFlags.DOTALL);
            result = head_regex.replace (result, -1, 0, "");

            // Remove body tags
            var body_regex = new Regex ("</?body[^>]*>", RegexCompileFlags.CASELESS);
            result = body_regex.replace (result, -1, 0, "");
        } catch (RegexError e) {
            warning ("Regex error stripping wrapper: %s", e.message);
        }

        return result;
    }
}
