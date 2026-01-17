vapi_dir = meson.current_source_dir()

sources = files(
    'Application.vala',
    'MainWindow.vala',
    'Models/Chat.vala',
    'Models/Message.vala',
    'Services/Database.vala',
    'Services/AIService.vala',
    'Services/MarkdownRenderer.vala',
    'Widgets/ChatSidebar.vala',
    'Widgets/ChatView.vala',
    'Widgets/MessageRow.vala',
    'Widgets/WelcomeView.vala',
    'Dialogs/SettingsDialog.vala',
)

executable(
    meson.project_name(),
    sources,
    dependencies: [
        dependency('granite-7', version: '>= 7.0.0'),
        dependency('gtk4'),
        dependency('libadwaita-1', version: '>= 1.0.0'),
        dependency('glib-2.0'),
        dependency('gobject-2.0'),
        dependency('sqlite3'),
        dependency('libsoup-3.0'),
        dependency('json-glib-1.0'),
        meson.get_compiler('c').find_library('cmark-gfm'),
    ],
    vala_args: ['--vapidir', vapi_dir, '--pkg', 'cmark-gfm'],
    install: true
)
