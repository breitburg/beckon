[CCode (cheader_filename = "cmark-gfm.h")]
namespace CmarkGfm {
    [CCode (cname = "CMARK_OPT_DEFAULT")]
    public const int OPT_DEFAULT;

    [CCode (cname = "CMARK_OPT_UNSAFE")]
    public const int OPT_UNSAFE;

    [CCode (cname = "cmark_markdown_to_html")]
    public string markdown_to_html (string text, size_t len, int options);
}
